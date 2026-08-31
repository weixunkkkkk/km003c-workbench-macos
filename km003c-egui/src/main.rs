mod connection;
mod i18n;
mod measurement;
mod offline_export;
mod offline_view;
mod pd_connection;
mod pd_decoder;
mod pd_trace_view;
mod preferences;
mod recording;
mod recording_import;
mod recording_session;
mod sleep_assertion;
mod theme;

use chrono::Utc;
use connection::ConnectionPhase;
use eframe::egui;
use egui_plot::{
    AxisHints, Corner, GridMark, HPlacement, Legend, Line, LineStyle, Plot, PlotBounds, PlotPoints, Points, Span, VLine,
};
use i18n::{APP_BUILD, APP_ID, APP_TITLE, APP_VERSION, Language};
use km003c_lib::uom::si::electric_potential::volt;
use km003c_lib::{
    AdcQueueSample, DeviceConfig, DeviceState, GraphSampleRate, KM003C, LogMetadata, OfflineLog, PdTrace,
    packet::{Attribute, AttributeSet},
    pd::{PdEvent, PdEventData, PdStatus},
};
use measurement::{MeasurementAccumulator, MeasurementSample, PlotMetric};
use offline_export::{OfflineExportEvent, OfflineExportTask};
use offline_view::{OfflineRecordingView, OfflineViewSample};
use pd_connection::PdConnectionTracker;
use pd_decoder::{DecodedPdEntry, PdCategory, PdContract, PdContractKind, PdDecoder, PowerProtocolState};
use pd_trace_view::{PdTraceCategory, PdTraceEntry, decode_trace};
use preferences::{AppPreferences, AutoCaptureMetric, AutoCaptureRule, DisplayFilter, WorkspaceTab};
use recording::{Recorder, RecordingEvent, RecordingFormat, RecordingMetadata, RecordingOffsets, RecordingSummary};
use recording_import::{ImportedRecording, RecordingImportEvent, RecordingImportTask};
use recording_session::{
    IntervalReason, RecordingSegmentMetadata, RecordingSessionManifestV1, RecordingSessionMetadataV1,
    RecordingTimeInterval, SessionState, discover_recoverable_sessions, merge_session_segments, read_sidecar,
    write_manifest, write_sidecar,
};
use sleep_assertion::IdleSleepAssertion;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

/// Keep a macOS unlock catch-up burst from monopolizing the egui thread.
/// Consecutive sample messages are coalesced into one recorder hand-off, and
/// any remaining backlog is handled on the next repaint.
const MAX_USB_MESSAGES_PER_FRAME: usize = 64;

/// Message from USB task to UI
#[derive(Debug, Clone)]
enum UsbMessage {
    /// Device connected and initialized
    Connected(Arc<DeviceState>),
    /// Connection failed
    ConnectionFailed(String),
    /// New AdcQueue samples received
    Samples(Vec<AdcQueueSample>),
    /// PD events received from device
    PdEvents(Vec<PdEvent>),
    /// PD status (CC line voltages)
    PdStatusUpdate(PdStatus),
    /// Firmware Type-C and protocol-engine trace
    PdTrace(PdTrace),
    /// Device offline-recording catalog
    OfflineCatalog(Vec<LogMetadata>),
    /// Complete selected offline recording
    OfflineLogDownloaded(OfflineLog),
    /// Offline catalog or download operation failed
    OfflineOperationFailed(String),
    /// Streaming started at given rate
    StreamingStarted(GraphSampleRate),
    /// Streaming stopped
    StreamingStopped,
    /// Error during streaming
    Error(String),
    /// Disconnected
    Disconnected,
}

/// Command from UI to USB task
#[derive(Debug, Clone)]
enum UsbCommand {
    /// Connect to device and start streaming
    Connect(GraphSampleRate, bool),
    /// Change sample rate (stops current streaming, starts with new rate)
    SetSampleRate(GraphSampleRate),
    /// Enable or disable firmware PD trace collection
    SetPdTraceEnabled(bool),
    /// Fetch the catalog of recordings stored by the device
    RequestOfflineCatalog,
    /// Download one catalog entry from device memory
    DownloadOfflineLog(LogMetadata),
    /// Stop streaming and disconnect
    Disconnect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlotSource {
    Live,
    Offline,
    Imported,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum SettingsPage {
    #[default]
    General,
    Recording,
    Chart,
    DataAndDevice,
    Diagnostics,
}

impl SettingsPage {
    const ALL: [Self; 5] = [
        Self::General,
        Self::Recording,
        Self::Chart,
        Self::DataAndDevice,
        Self::Diagnostics,
    ];

    const fn localized_label(self, language: Language) -> &'static str {
        match self {
            Self::General => language.pick("通用设置", "General"),
            Self::Recording => language.pick("录制与恢复", "Recording & recovery"),
            Self::Chart => language.pick("图表与显示", "Chart & display"),
            Self::DataAndDevice => language.pick("数据与设备", "Data & device"),
            Self::Diagnostics => language.pick("诊断与关于", "Diagnostics & about"),
        }
    }

    const fn localized_description(self, language: Language) -> &'static str {
        match self {
            Self::General => language.pick(
                "设置界面语言、当前数据源和 KM003C 连接选项。",
                "Choose the interface language, data source, and KM003C connection options.",
            ),
            Self::Recording => language.pick(
                "管理录制格式、睡眠保护、自动暂停和待恢复会话。",
                "Manage recording format, sleep protection, automatic pause, and recoverable sessions.",
            ),
            Self::Chart => language.pick(
                "调整时间范围、屏幕降噪和高级分析曲线。",
                "Adjust the time range, display smoothing, and advanced-analysis traces.",
            ),
            Self::DataAndDevice => language.pick(
                "检查数据完整度，并下载 KM003C 内置存储中的记录。",
                "Inspect data integrity and download recordings stored on the KM003C.",
            ),
            Self::Diagnostics => language.pick(
                "查看版本、日志位置、开源许可和项目来源。",
                "View version details, log location, licenses, and project sources.",
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct SettingsLayoutMetrics {
    window_size: egui::Vec2,
    navigation_width: f32,
    column_gap: f32,
    content_width: f32,
    scroll_height: f32,
    footer_height: f32,
}

impl SettingsLayoutMetrics {
    const VIEWPORT_MARGIN: f32 = 48.0;
    const MIN_WINDOW_WIDTH: f32 = 820.0;
    const MAX_WINDOW_WIDTH: f32 = 900.0;
    const MIN_WINDOW_HEIGHT: f32 = 560.0;
    const MAX_WINDOW_HEIGHT: f32 = 700.0;
    const NAVIGATION_WIDTH: f32 = 184.0;
    const COLUMN_GAP: f32 = 12.0;
    const FOOTER_HEIGHT: f32 = 44.0;
    const WINDOW_HORIZONTAL_CHROME: f32 = 24.0;
    const WINDOW_VERTICAL_CHROME: f32 = 44.0;
    const PAGE_HEADER_HEIGHT: f32 = 62.0;

    fn for_content_rect(content_rect: egui::Rect) -> Self {
        let maximum_available_width = (content_rect.width() - 24.0).max(0.0);
        let maximum_available_height = (content_rect.height() - 24.0).max(0.0);
        let window_width = (content_rect.width() - Self::VIEWPORT_MARGIN)
            .clamp(Self::MIN_WINDOW_WIDTH, Self::MAX_WINDOW_WIDTH)
            .min(maximum_available_width);
        let window_height = (content_rect.height() - Self::VIEWPORT_MARGIN)
            .clamp(Self::MIN_WINDOW_HEIGHT, Self::MAX_WINDOW_HEIGHT)
            .min(maximum_available_height);
        let content_width =
            (window_width - Self::WINDOW_HORIZONTAL_CHROME - Self::NAVIGATION_WIDTH - Self::COLUMN_GAP).max(0.0);
        let available_body_height = (window_height - Self::WINDOW_VERTICAL_CHROME).max(0.0);
        let scroll_height = (available_body_height - Self::PAGE_HEADER_HEIGHT - Self::FOOTER_HEIGHT).max(0.0);

        Self {
            window_size: egui::vec2(window_width, window_height),
            navigation_width: Self::NAVIGATION_WIDTH,
            column_gap: Self::COLUMN_GAP,
            content_width,
            scroll_height,
            footer_height: Self::FOOTER_HEIGHT,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolbarDensity {
    Full,
    Compact,
    Narrow,
}

fn toolbar_density(width: f32) -> ToolbarDensity {
    if width < 1120.0 {
        ToolbarDensity::Narrow
    } else if width < 1360.0 {
        ToolbarDensity::Compact
    } else {
        ToolbarDensity::Full
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum RecordingPhase {
    #[default]
    Idle,
    Recording,
    Paused,
    Finalizing,
    Saved,
    Interrupted,
    WaitingForReconnect,
    Recovering,
}

impl RecordingPhase {
    fn localized_label(self, language: Language) -> &'static str {
        match self {
            Self::Idle => language.pick("未记录", "Not recording"),
            Self::Recording => language.pick("记录中", "Recording"),
            Self::Paused => language.pick("已暂停", "Paused"),
            Self::Finalizing => language.pick("正在保存", "Saving"),
            Self::Saved => language.pick("已保存", "Saved"),
            Self::Interrupted => language.pick("记录中断", "Interrupted"),
            Self::WaitingForReconnect => language.pick("等待重连", "Waiting to reconnect"),
            Self::Recovering => language.pick("正在恢复", "Recovering"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PauseReason {
    Manual,
    Automatic(AutoCaptureMetric),
    UsbDisconnected,
}

impl PauseReason {
    const fn interval_reason(self) -> IntervalReason {
        match self {
            Self::Manual => IntervalReason::ManualPause,
            Self::Automatic(_) => IntervalReason::AutomaticPause,
            Self::UsbDisconnected => IntervalReason::UsbDisconnected,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct AxisScale {
    maximum: f64,
}

impl AxisScale {
    fn from_visible_max(value: f64) -> Self {
        Self {
            maximum: nice_axis_ceiling(value.max(0.0) * 1.06),
        }
    }

    fn normalize(self, value: f64) -> f64 {
        (value / self.maximum).clamp(0.0, 1.0)
    }

    #[cfg(test)]
    fn denormalize(self, value: f64) -> f64 {
        value * self.maximum
    }

    fn presentation(self, unit: MeasurementUnit) -> EngineeringPresentation {
        EngineeringPresentation::for_maximum(self.maximum, unit)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MeasurementUnit {
    Voltage,
    Current,
    Power,
}

impl MeasurementUnit {
    const fn symbol(self) -> &'static str {
        match self {
            Self::Voltage => "V",
            Self::Current => "A",
            Self::Power => "W",
        }
    }

    const fn milli_symbol(self) -> &'static str {
        match self {
            Self::Voltage => "mV",
            Self::Current => "mA",
            Self::Power => "mW",
        }
    }

    const fn micro_symbol(self) -> &'static str {
        match self {
            Self::Voltage => "µV",
            Self::Current => "µA",
            Self::Power => "µW",
        }
    }
}

/// Human-readable engineering-unit scale used by axes, cards and cursor values.
///
/// Keeping the plot coordinates in base V/A/W avoids changing the recording
/// contract. Only the presentation changes, so a 0.004 A range is labelled in
/// mA instead of collapsing every tick to `0.00 A`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct EngineeringPresentation {
    multiplier: f64,
    symbol: &'static str,
    decimals: usize,
}

impl EngineeringPresentation {
    fn for_maximum(maximum: f64, unit: MeasurementUnit) -> Self {
        let maximum = maximum.abs();
        let (multiplier, symbol) = if maximum > 0.0 && maximum < 0.0001 {
            (1_000_000.0, unit.micro_symbol())
        } else if maximum > 0.0 && maximum < 0.1 {
            (1_000.0, unit.milli_symbol())
        } else {
            (1.0, unit.symbol())
        };
        let scaled_tick = maximum * multiplier / 5.0;
        let decimals = if scaled_tick >= 100.0 {
            0
        } else if scaled_tick >= 10.0 {
            1
        } else if scaled_tick >= 1.0 {
            2
        } else if scaled_tick >= 0.1 {
            3
        } else if scaled_tick >= 0.01 {
            4
        } else {
            5
        };
        Self {
            multiplier,
            symbol,
            decimals,
        }
    }

    fn for_value(value: f64, unit: MeasurementUnit) -> Self {
        Self::for_maximum(value.abs(), unit)
    }

    fn format_value(self, value: f64) -> String {
        format!("{:.*}", self.decimals, value * self.multiplier)
    }
}

fn nice_axis_ceiling(value: f64) -> f64 {
    if !value.is_finite() || value <= 0.0 {
        return 1.0;
    }
    let magnitude = 10_f64.powf(value.log10().floor());
    let normalized = value / magnitude;
    let nice = if normalized <= 1.0 {
        1.0
    } else if normalized <= 2.0 {
        2.0
    } else if normalized <= 5.0 {
        5.0
    } else {
        10.0
    };
    nice * magnitude
}

fn uses_compact_monitor_layout(width: f32, height: f32) -> bool {
    // The measurement rail and chart receive the space left after both
    // toolbars. Switch based on that usable area so common 1160 px windows do
    // not stay in the full desktop layout merely because the outer viewport is
    // a few pixels above an arbitrary breakpoint.
    width < 1240.0 || height < 620.0
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct NavigatorSelection {
    start_seconds: f64,
    end_seconds: f64,
}

impl NavigatorSelection {
    fn clamped(self, full_end: f64) -> Self {
        let full_end = full_end.max(0.001);
        let width = (self.end_seconds - self.start_seconds).clamp(0.001, full_end);
        let start = self.start_seconds.clamp(0.0, (full_end - width).max(0.0));
        Self {
            start_seconds: start,
            end_seconds: (start + width).min(full_end),
        }
    }

    fn width(self) -> f64 {
        self.end_seconds - self.start_seconds
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NavigatorDrag {
    Start,
    Range,
    End,
}

#[derive(Debug, Default, Clone, Copy)]
struct ChartViewport {
    selection: Option<NavigatorSelection>,
    drag: Option<NavigatorDrag>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum ChartFollowMode {
    /// Keep the viewport anchored to zero and grow it with the recording.
    FullSession,
    /// Keep a configured-width window anchored to the newest sample.
    #[default]
    LatestWindow,
    /// Preserve the user's dragged or zoomed viewport while acquisition continues.
    Manual,
}

impl ChartFollowMode {
    const fn is_following(self) -> bool {
        !matches!(self, Self::Manual)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct NavigatorOverviewPoint {
    time_seconds: f64,
    values: [f64; 3],
    minimums: [f64; 3],
    maximums: [f64; 3],
}

/// A bounded, progressively compacted overview of the entire live session.
///
/// The detailed `data_points` ring stays optimized for the current viewport,
/// while this store keeps the minimap useful for hours-long 1000 SPS runs.
/// It retains per-bucket averages plus extrema and is never used for
/// recording/export. The extrema keep short 1000 SPS transients visible after
/// hours of progressive compaction.
#[derive(Debug)]
struct NavigatorHistory {
    points: Vec<NavigatorOverviewPoint>,
    bucket_width_seconds: f64,
    active_bucket: Option<u64>,
    sums: [f64; 3],
    minimums: [f64; 3],
    maximums: [f64; 3],
    count: u32,
    max_points: usize,
}

impl Default for NavigatorHistory {
    fn default() -> Self {
        Self::with_limit(120_000)
    }
}

impl NavigatorHistory {
    fn with_limit(max_points: usize) -> Self {
        Self {
            points: Vec::new(),
            bucket_width_seconds: 0.1,
            active_bucket: None,
            sums: [0.0; 3],
            minimums: [f64::INFINITY; 3],
            maximums: [f64::NEG_INFINITY; 3],
            count: 0,
            max_points: max_points.max(4),
        }
    }

    fn push(&mut self, sample: MeasurementSample) {
        self.push_values(
            sample.elapsed_seconds(),
            [
                sample.vbus_uv as f64 / 1_000_000.0,
                (sample.ibus_ua as f64 / 1_000_000.0).abs(),
                (sample.power_uw as f64 / 1_000_000.0).abs(),
            ],
        );
    }

    fn push_values(&mut self, time_seconds: f64, values: [f64; 3]) {
        if !time_seconds.is_finite() || values.iter().any(|value| !value.is_finite()) {
            return;
        }
        let bucket = (time_seconds.max(0.0) / self.bucket_width_seconds).floor() as u64;
        if self.active_bucket.is_some_and(|active| bucket < active) {
            self.clear();
        }

        if self.active_bucket == Some(bucket) {
            for (index, value) in values.into_iter().enumerate() {
                self.sums[index] += value;
                self.minimums[index] = self.minimums[index].min(value);
                self.maximums[index] = self.maximums[index].max(value);
            }
            self.count = self.count.saturating_add(1);
            if let Some(point) = self.points.last_mut() {
                point.time_seconds = time_seconds;
                for (output, sum) in point.values.iter_mut().zip(self.sums) {
                    *output = sum / f64::from(self.count);
                }
                point.minimums = self.minimums;
                point.maximums = self.maximums;
            }
            return;
        }

        self.active_bucket = Some(bucket);
        self.sums = values;
        self.minimums = values;
        self.maximums = values;
        self.count = 1;
        self.points.push(NavigatorOverviewPoint {
            time_seconds,
            values,
            minimums: values,
            maximums: values,
        });
        if self.points.len() > self.max_points {
            self.compact();
        }
    }

    fn compact(&mut self) {
        let mut compacted = Vec::with_capacity(self.points.len().div_ceil(2));
        for pair in self.points.chunks(2) {
            let last = pair[pair.len() - 1];
            let mut values = [0.0; 3];
            let mut minimums = [f64::INFINITY; 3];
            let mut maximums = [f64::NEG_INFINITY; 3];
            for point in pair {
                for index in 0..3 {
                    values[index] += point.values[index];
                    minimums[index] = minimums[index].min(point.minimums[index]);
                    maximums[index] = maximums[index].max(point.maximums[index]);
                }
            }
            for value in &mut values {
                *value /= pair.len() as f64;
            }
            compacted.push(NavigatorOverviewPoint {
                time_seconds: last.time_seconds,
                values,
                minimums,
                maximums,
            });
        }
        self.points = compacted;
        self.bucket_width_seconds *= 2.0;
        if let Some(last) = self.points.last().copied() {
            self.active_bucket = Some((last.time_seconds / self.bucket_width_seconds).floor() as u64);
            self.sums = last.values;
            self.minimums = last.minimums;
            self.maximums = last.maximums;
            self.count = 1;
        }
    }

    fn clear(&mut self) {
        let max_points = self.max_points;
        *self = Self::with_limit(max_points);
    }

    fn readout_at(&self, target: f64) -> Option<CursorReadout> {
        nearest_time_index(self.points.len(), target, |index| self.points[index].time_seconds).map(|index| {
            let point = self.points[index];
            CursorReadout {
                time_seconds: point.time_seconds,
                voltage: point.values[0],
                current: point.values[1],
                power: point.values[2],
                approximate: true,
            }
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct PauseInterval {
    start_seconds: f64,
    end_seconds: f64,
}

enum PdTimelineEntry<'a> {
    Protocol(&'a DecodedPdEntry),
    FirmwareTrace(&'a PdTraceEntry),
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct CursorReadout {
    time_seconds: f64,
    voltage: f64,
    current: f64,
    power: f64,
    approximate: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct AccumulatedReadout {
    cumulative_energy_uwh: f64,
    capacity_uah: f64,
    net_energy_uwh: f64,
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
struct ScopeStatistics {
    duration_seconds: f64,
    cumulative_energy_uwh: f64,
    capacity_uah: f64,
    points: u64,
    approximate: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct MetricStatistics {
    minimum: f64,
    average: f64,
    maximum: f64,
}

#[derive(Debug, Clone, Copy)]
struct RunningMetricStatistics {
    count: u64,
    minimum: f64,
    maximum: f64,
    sum: f64,
}

impl Default for RunningMetricStatistics {
    fn default() -> Self {
        Self {
            count: 0,
            minimum: f64::INFINITY,
            maximum: f64::NEG_INFINITY,
            sum: 0.0,
        }
    }
}

impl RunningMetricStatistics {
    fn push(&mut self, value: f64) {
        if !value.is_finite() {
            return;
        }
        self.count = self.count.saturating_add(1);
        self.minimum = self.minimum.min(value);
        self.maximum = self.maximum.max(value);
        self.sum += value;
    }

    fn readout(self) -> Option<MetricStatistics> {
        (self.count > 0).then_some(MetricStatistics {
            minimum: self.minimum,
            average: self.sum / self.count as f64,
            maximum: self.maximum,
        })
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct RecordingSessionStatistics {
    voltage: RunningMetricStatistics,
    current: RunningMetricStatistics,
    power: RunningMetricStatistics,
}

impl RecordingSessionStatistics {
    fn push(&mut self, sample: MeasurementSample) {
        self.voltage.push(sample.vbus_uv as f64 / 1_000_000.0);
        self.current.push((sample.ibus_ua as f64 / 1_000_000.0).abs());
        self.power.push((sample.power_uw as f64 / 1_000_000.0).abs());
    }

    fn from_measurements<'a>(samples: impl IntoIterator<Item = &'a MeasurementSample>) -> Self {
        let mut statistics = Self::default();
        for sample in samples {
            statistics.push(*sample);
        }
        statistics
    }
}

fn nearest_time_index(len: usize, target: f64, mut time_at: impl FnMut(usize) -> f64) -> Option<usize> {
    if len == 0 {
        return None;
    }

    let target = if target.is_finite() { target.max(0.0) } else { 0.0 };
    let mut low = 0;
    let mut high = len;
    while low < high {
        let middle = low + (high - low) / 2;
        if time_at(middle) < target {
            low = middle + 1;
        } else {
            high = middle;
        }
    }

    match (low.checked_sub(1), (low < len).then_some(low)) {
        (Some(before), Some(after)) => {
            let before_distance = (time_at(before) - target).abs();
            let after_distance = (time_at(after) - target).abs();
            Some(if before_distance <= after_distance {
                before
            } else {
                after
            })
        }
        (Some(before), None) => Some(before),
        (None, Some(after)) => Some(after),
        (None, None) => None,
    }
}

/// Reduce the number of rendered points while retaining the lowest and
/// highest sample in every time bucket. This preserves short spikes that a
/// simple every-Nth decimator would hide.
fn min_max_downsample(points: Vec<[f64; 2]>, max_output_points: usize) -> Vec<[f64; 2]> {
    let max_output_points = max_output_points.max(4);
    if points.len() <= max_output_points {
        return points;
    }

    let first = points[0];
    let last = points[points.len() - 1];
    let bucket_count = (max_output_points.saturating_sub(2) / 2).max(1);
    let bucket_size = points.len().div_ceil(bucket_count);
    let mut reduced = Vec::with_capacity(max_output_points + 2);
    for bucket in points.chunks(bucket_size) {
        let (mut minimum_index, mut maximum_index) = (0, 0);
        for (index, point) in bucket.iter().enumerate().skip(1) {
            if point[1] < bucket[minimum_index][1] {
                minimum_index = index;
            }
            if point[1] > bucket[maximum_index][1] {
                maximum_index = index;
            }
        }
        match minimum_index.cmp(&maximum_index) {
            std::cmp::Ordering::Less => {
                reduced.push(bucket[minimum_index]);
                reduced.push(bucket[maximum_index]);
            }
            std::cmp::Ordering::Greater => {
                reduced.push(bucket[maximum_index]);
                reduced.push(bucket[minimum_index]);
            }
            std::cmp::Ordering::Equal => reduced.push(bucket[minimum_index]),
        }
    }
    if reduced.first().is_none_or(|point| *point != first) {
        reduced.insert(0, first);
    }
    if reduced.last().is_none_or(|point| *point != last) {
        reduced.push(last);
    }
    reduced
}

fn apply_display_filter(mut points: Vec<[f64; 2]>, filter: DisplayFilter) -> Vec<[f64; 2]> {
    if filter == DisplayFilter::Raw || points.len() < 3 {
        return points;
    }

    // A five-point median makes low-current idle traces readable without
    // changing the measured data. It rejects narrow spikes while preserving
    // sustained steps much better than an averaging filter. Filtering stays
    // in the presentation layer; cursor values, statistics and exports remain
    // raw. Three points are used only at the two edges of a short capture.
    let source = points.iter().map(|point| point[1]).collect::<Vec<_>>();
    if points.len() < 5 {
        for index in 1..points.len() - 1 {
            let mut window = [source[index - 1], source[index], source[index + 1]];
            window.sort_by(f64::total_cmp);
            points[index][1] = window[1];
        }
    } else {
        for index in 2..points.len() - 2 {
            let mut window = [
                source[index - 2],
                source[index - 1],
                source[index],
                source[index + 1],
                source[index + 2],
            ];
            window.sort_by(f64::total_cmp);
            points[index][1] = window[2];
        }
    }
    points
}

trait ScopeSample {
    fn energy_throughput_uwh(&self) -> f64;
    fn charge_throughput_uah(&self) -> f64;
}

impl ScopeSample for MeasurementSample {
    fn energy_throughput_uwh(&self) -> f64 {
        self.energy_throughput_uwh
    }

    fn charge_throughput_uah(&self) -> f64 {
        self.charge_throughput_uah
    }
}

impl ScopeSample for OfflineViewSample {
    fn energy_throughput_uwh(&self) -> f64 {
        self.energy_throughput_uwh
    }

    fn charge_throughput_uah(&self) -> f64 {
        self.charge_throughput_uah
    }
}

fn calculate_scope_statistics<'a, T: ScopeSample + 'a>(
    samples: impl Iterator<Item = (f64, &'a T)>,
    selection: NavigatorSelection,
    excluded_intervals: &[PauseInterval],
) -> ScopeStatistics {
    let mut result = ScopeStatistics::default();
    let mut previous: Option<(f64, &'a T)> = None;

    for (time, sample) in samples {
        if time < selection.start_seconds || time > selection.end_seconds {
            continue;
        }
        let excluded = excluded_intervals
            .iter()
            .any(|interval| time >= interval.start_seconds && time <= interval.end_seconds);
        if excluded {
            previous = None;
            continue;
        }

        result.points = result.points.saturating_add(1);
        if let Some((previous_time, previous_sample)) = previous {
            result.duration_seconds += (time - previous_time).max(0.0);
            result.cumulative_energy_uwh +=
                (sample.energy_throughput_uwh() - previous_sample.energy_throughput_uwh()).max(0.0);
            result.capacity_uah += (sample.charge_throughput_uah() - previous_sample.charge_throughput_uah()).max(0.0);
        }
        previous = Some((time, sample));
    }

    result
}

fn format_recording_duration(duration: Duration) -> String {
    let total_millis = duration.as_millis();
    let hours = total_millis / 3_600_000;
    let minutes = (total_millis / 60_000) % 60;
    let seconds = (total_millis / 1_000) % 60;
    let tenths = (total_millis % 1_000) / 100;
    format!("{hours:02}:{minutes:02}:{seconds:02}.{tenths}")
}

fn format_cumulative_energy(energy_uwh: f64) -> String {
    let energy_uwh = energy_uwh.max(0.0);
    if energy_uwh >= 1_000_000.0 {
        format!("{:.4} Wh", energy_uwh / 1_000_000.0)
    } else if energy_uwh >= 1_000.0 {
        format!("{:.3} mWh", energy_uwh / 1_000.0)
    } else {
        format!("{energy_uwh:.1} µWh")
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct EnergyPresentation {
    divisor: f64,
    symbol: &'static str,
    decimals: usize,
}

impl EnergyPresentation {
    fn for_values(values_uwh: impl IntoIterator<Item = f64>) -> Self {
        let maximum = values_uwh.into_iter().map(f64::abs).fold(0.0_f64, f64::max);
        if maximum >= 1_000_000.0 {
            Self {
                divisor: 1_000_000.0,
                symbol: "Wh",
                decimals: 4,
            }
        } else if maximum >= 1_000.0 {
            Self {
                divisor: 1_000.0,
                symbol: "mWh",
                decimals: 3,
            }
        } else {
            Self {
                divisor: 1.0,
                symbol: "µWh",
                decimals: 1,
            }
        }
    }

    fn format(self, value_uwh: f64) -> String {
        format!("{:.*} {}", self.decimals, value_uwh / self.divisor, self.symbol)
    }

    fn format_directional(self, value_uwh: f64) -> String {
        let direction = if value_uwh > f64::EPSILON {
            "↑"
        } else if value_uwh < -f64::EPSILON {
            "↓"
        } else {
            "•"
        };
        format!("{direction} {}", self.format(value_uwh.abs()))
    }
}

fn format_capacity(capacity_uah: f64) -> String {
    if capacity_uah.abs() >= 1_000_000.0 {
        format!("{:.4} Ah", capacity_uah / 1_000_000.0)
    } else {
        format!("{:.3} mAh", capacity_uah / 1_000.0)
    }
}

fn format_plot_time(seconds: f64) -> String {
    if seconds.is_finite() && seconds >= 0.0 {
        format_recording_duration(Duration::from_secs_f64(seconds))
    } else {
        format_recording_duration(Duration::ZERO)
    }
}

fn auto_capture_value_micro(metric: AutoCaptureMetric, sample: MeasurementSample) -> u64 {
    match metric {
        AutoCaptureMetric::Power => sample.power_uw.unsigned_abs(),
        AutoCaptureMetric::Current => sample.ibus_ua.unsigned_abs(),
        AutoCaptureMetric::Voltage => sample.vbus_uv.unsigned_abs(),
    }
}

fn auto_capture_hysteresis_micro(rule: AutoCaptureRule) -> u64 {
    let threshold = u64::from(rule.threshold_milli).saturating_mul(1_000);
    match rule.metric {
        AutoCaptureMetric::Power => (threshold / 20).max(50_000),
        AutoCaptureMetric::Current => (threshold / 20).max(20_000),
        AutoCaptureMetric::Voltage => (threshold / 50).max(100_000),
    }
}

fn apply_recording_offsets(mut sample: MeasurementSample, offsets: RecordingOffsets) -> MeasurementSample {
    sample.elapsed_us = sample.elapsed_us.saturating_add(offsets.elapsed_us);
    sample.sample_index = sample.sample_index.saturating_add(offsets.sample_index);
    sample.charge_uah += offsets.charge_uah;
    sample.energy_uwh += offsets.energy_uwh;
    sample.charge_throughput_uah += offsets.charge_throughput_uah;
    sample.energy_throughput_uwh += offsets.energy_throughput_uwh;
    sample.cumulative_missing_samples = sample
        .cumulative_missing_samples
        .saturating_add(offsets.cumulative_missing_samples);
    sample.cumulative_interpolated_duration_us = sample
        .cumulative_interpolated_duration_us
        .saturating_add(offsets.cumulative_interpolated_duration_us);
    sample.cumulative_discarded_sequence_samples = sample
        .cumulative_discarded_sequence_samples
        .saturating_add(offsets.cumulative_discarded_sequence_samples);
    sample
}

fn application_recordings_directory() -> PathBuf {
    if let Some(storage_root) = std::env::var_os("KM003C_STORAGE_ROOT") {
        return PathBuf::from(storage_root).join("Recordings");
    }
    std::env::var_os("HOME").map_or_else(
        || std::env::temp_dir().join(APP_ID).join("Recordings"),
        |home| {
            PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join(APP_ID)
                .join("Recordings")
        },
    )
}

fn stage_pending_recording(source: &Path, destination: &Path) -> Result<(), String> {
    if source == destination {
        return Ok(());
    }
    if let Some(parent) = destination.parent()
        && !parent.exists()
    {
        return Err(format!("目标目录不存在：{}", parent.display()));
    }
    std::fs::copy(source, destination).map_err(|error| format!("保存录制失败：{error}"))?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .open(destination)
        .map_err(|error| format!("无法校验保存文件：{error}"))?;
    file.sync_all().map_err(|error| format!("无法同步保存文件：{error}"))
}

impl PdTimelineEntry<'_> {
    fn timestamp_seconds(&self) -> f64 {
        match self {
            Self::Protocol(entry) => entry.timestamp_seconds,
            Self::FirmwareTrace(entry) => entry.timestamp_seconds,
        }
    }
}

fn pd_timeline_entries<'a>(
    protocol_log: &'a VecDeque<DecodedPdEntry>,
    trace_log: &'a VecDeque<PdTraceEntry>,
    show_protocol: bool,
    show_trace: bool,
) -> Vec<PdTimelineEntry<'a>> {
    let mut timeline = Vec::with_capacity(protocol_log.len() + trace_log.len());
    if show_protocol {
        timeline.extend(protocol_log.iter().map(PdTimelineEntry::Protocol));
    }
    if show_trace {
        timeline.extend(trace_log.iter().map(PdTimelineEntry::FirmwareTrace));
    }
    timeline.sort_by(|left, right| left.timestamp_seconds().total_cmp(&right.timestamp_seconds()));
    timeline
}

/// Sample rate options for the UI
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum SampleRateOption {
    Sps2,
    Sps10,
    Sps50,
    Sps1000,
}

impl SampleRateOption {
    fn to_graph_rate(self) -> GraphSampleRate {
        match self {
            Self::Sps2 => GraphSampleRate::Sps2,
            Self::Sps10 => GraphSampleRate::Sps10,
            Self::Sps50 => GraphSampleRate::Sps50,
            Self::Sps1000 => GraphSampleRate::Sps1000,
        }
    }

    fn from_graph_rate(rate: GraphSampleRate) -> Self {
        match rate {
            GraphSampleRate::Sps2 => Self::Sps2,
            GraphSampleRate::Sps10 => Self::Sps10,
            GraphSampleRate::Sps50 => Self::Sps50,
            GraphSampleRate::Sps1000 => Self::Sps1000,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Sps2 => "2 SPS",
            Self::Sps10 => "10 SPS",
            Self::Sps50 => "50 SPS",
            Self::Sps1000 => "1000 SPS",
        }
    }

    const fn hertz(self) -> u32 {
        match self {
            Self::Sps2 => 2,
            Self::Sps10 => 10,
            Self::Sps50 => 50,
            Self::Sps1000 => 1_000,
        }
    }

    fn all() -> &'static [Self] {
        &[Self::Sps2, Self::Sps10, Self::Sps50, Self::Sps1000]
    }
}

/// Time window for plot display
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum TimeWindow {
    Sec2,
    Sec10,
    Sec30,
    Min1,
    Min5,
    All,
}

impl TimeWindow {
    fn seconds(self) -> Option<f64> {
        match self {
            Self::Sec2 => Some(2.0),
            Self::Sec10 => Some(10.0),
            Self::Sec30 => Some(30.0),
            Self::Min1 => Some(60.0),
            Self::Min5 => Some(300.0),
            Self::All => None,
        }
    }

    fn localized_label(self, language: Language) -> &'static str {
        match self {
            Self::Sec2 => language.pick("2 秒", "2 seconds"),
            Self::Sec10 => language.pick("10 秒", "10 seconds"),
            Self::Sec30 => language.pick("30 秒", "30 seconds"),
            Self::Min1 => language.pick("1 分钟", "1 minute"),
            Self::Min5 => language.pick("5 分钟", "5 minutes"),
            Self::All => language.pick("全部", "All"),
        }
    }

    fn all() -> &'static [Self] {
        &[Self::Sec2, Self::Sec10, Self::Sec30, Self::Min1, Self::Min5, Self::All]
    }
}

struct PowerMonitorApp {
    /// User-facing UI language. Protocol names and engineering units remain standardized.
    language: Language,
    /// Complete live samples retained for plotting
    data_points: VecDeque<MeasurementSample>,
    /// Progressively compacted whole-session overview for the navigator.
    navigator_history: NavigatorHistory,
    /// Unwraps device time and integrates charge and energy
    measurement_accumulator: MeasurementAccumulator,
    /// Applied only when continuing a persisted session after application
    /// restart, so newly decoded device samples join the existing timeline.
    measurement_resume_offsets: Option<RecordingOffsets>,
    /// Receiver for USB messages
    usb_receiver: mpsc::UnboundedReceiver<UsbMessage>,
    /// Sender for commands to USB task
    cmd_sender: mpsc::UnboundedSender<UsbCommand>,
    /// Device state (available after connection)
    device_state: Option<Arc<DeviceState>>,
    /// Connection status string
    status: String,
    /// Typed phase used for status color, retry policy and accessibility text.
    phase: ConnectionPhase,
    /// Next safe automatic retry after a DeviceNotFound result.
    next_retry_at: Instant,
    /// Keep the original USB error in the log while showing a concise action.
    last_connection_error: Option<String>,
    /// Distinguishes an intentional Disconnect button press from a cable pull.
    disconnect_requested: bool,
    disconnect_confirmation: bool,
    clear_data_confirmation: bool,
    /// Deterministic UI-only data source for screenshots and smoke tests.
    demo_mode: bool,
    demo_started: Instant,
    demo_last_tick: Instant,
    demo_sequence: u16,
    /// Is streaming active
    streaming: bool,
    /// Current sample rate (synced with device)
    current_rate: SampleRateOption,
    /// Selected sample rate in UI (may differ while changing)
    selected_rate: SampleRateOption,
    /// Time window for plot display
    time_window: TimeWindow,
    /// Maximum data points to keep (safety cap)
    max_points: usize,
    /// Total samples received
    total_samples: u64,
    /// Dropped sample count
    dropped_samples: u64,
    /// Duplicate, stale, or invalid-sequence samples excluded from measurements
    discarded_sequence_samples: u64,
    /// Current readings for display
    current_voltage: f64,
    current_current: f64,
    current_power: f64,
    /// Min/average/max values for samples actually written to the current or
    /// most recently completed recording. Live samples received while paused
    /// are intentionally excluded.
    recording_statistics: RecordingSessionStatistics,
    /// Metric selected independently in the advanced analysis window.
    plot_metrics: [PlotMetric; 3],
    /// Monitor, PD analysis, and transient panel state.
    active_tab: WorkspaceTab,
    settings_open: bool,
    settings_page: SettingsPage,
    advanced_analysis_open: bool,
    visible_series: [bool; 3],
    chart_follow_mode: ChartFollowMode,
    display_filter: DisplayFilter,
    chart_viewport: ChartViewport,
    /// Absolute device-session time represented by 00:00:00.0 on the live
    /// plot. Starting a recording moves this origin without touching samples.
    live_plot_origin_seconds: f64,
    /// Sample used only as the recorder's zero point. It is drawn at t=0 but
    /// is not itself a row in the new recording, so window point counts omit it.
    live_plot_origin_sample_index: Option<u64>,
    /// Preferred output format for live capture
    recording_format: RecordingFormat,
    /// Active or finalizing background recorder
    recorder: Option<Recorder>,
    /// Finalized segment writers are polled independently while the next
    /// segment can already accept samples.
    finalizing_segments: Vec<FinalizingSegment>,
    recording_session_directory: Option<PathBuf>,
    recording_manifest: Option<RecordingSessionManifestV1>,
    active_segment_start: RecordingOffsets,
    recording_continuation: RecordingOffsets,
    next_segment_index: u32,
    /// Last recorder status shown to the user
    recording_status: String,
    /// Summary of the last completed recording
    last_recording: Option<RecordingSummary>,
    /// Wall-clock metadata for the active logical recording. It is stored in
    /// a sidecar so the stable 23-column CSV/Parquet schema remains unchanged.
    recording_session_metadata: Option<RecordingSessionMetadataV1>,
    /// Metadata retained with the last saved recording for the left rail and
    /// import-style summary header.
    last_recording_metadata: Option<RecordingSessionMetadataV1>,
    recording_phase: RecordingPhase,
    pending_save_destination: Option<PathBuf>,
    pause_intervals: Vec<PauseInterval>,
    active_pause_started_at: Option<f64>,
    /// Whether the current recorder represents a user recording session.
    /// Buffer/offline exports use the same writer but do not use this clock.
    recording_session: bool,
    /// User requested pause state. Samples continue to feed the live plots,
    /// but are not sent to the recording writer while this is true.
    recording_paused: bool,
    /// Wall-clock time accumulated by the current recording, excluding pauses.
    recording_started_at: Option<Instant>,
    recording_elapsed_before_pause: Duration,
    /// Cumulative throughput energy at the start of the current segment. It is
    /// moved forward when pausing so energy accumulated during a pause is not
    /// counted in the saved segment.
    recording_energy_origin_uwh: f64,
    /// Energy accumulated by completed (pre-pause) recording segments.
    recording_energy_completed_uwh: f64,
    recording_total_energy_uwh: f64,
    recording_capacity_origin_uah: f64,
    recording_capacity_completed_uah: f64,
    recording_total_capacity_uah: f64,
    recording_net_energy_origin_uwh: f64,
    recording_net_energy_completed_uwh: f64,
    recording_net_energy_uwh: f64,
    /// Frozen values shown after a recording has been saved.
    last_recording_duration: Option<Duration>,
    last_recording_energy_uwh: Option<f64>,
    last_recording_capacity_uah: Option<f64>,
    last_recording_net_energy_uwh: Option<f64>,
    /// Optional low-power auto-pause. It is disabled by default and only
    /// pauses after the threshold has been continuously satisfied.
    auto_pause_enabled: bool,
    auto_capture_metric: AutoCaptureMetric,
    auto_pause_threshold_mw: u32,
    auto_pause_delay_ms: u32,
    auto_pause_below_since_us: Option<u64>,
    auto_resume_above_since_us: Option<u64>,
    pause_reason: Option<PauseReason>,
    /// Device-side offline recording catalog
    offline_catalog: Vec<LogMetadata>,
    /// Selected catalog row
    offline_selected: Option<usize>,
    /// Downloaded offline recording and plot data
    offline_view: Option<Arc<OfflineRecordingView>>,
    /// Device identity retained with the downloaded recording for export
    offline_device_metadata: Option<RecordingMetadata>,
    /// Whether a device catalog or download operation is running
    offline_busy: bool,
    /// Offline browser and export status
    offline_status: String,
    /// Background export of a downloaded offline recording
    offline_export: Option<OfflineExportTask>,
    /// Imported desktop CSV/Parquet recording and background loader.
    imported_recording: Option<ImportedRecording>,
    recording_import: Option<RecordingImportTask>,
    import_status: String,
    /// Data source currently rendered by the monitor and advanced plots.
    plot_source: PlotSource,
    /// Synchronized V/A/W values at the last plot cursor position.
    cursor_readout: Option<CursorReadout>,
    /// Keeps the last cursor table visible while the time viewport moves.
    cursor_pinned: bool,
    /// Reset all linked plot bounds on the next frame after the user asks to
    /// return to the live window or changes the plotted source/window.
    reset_plots_requested: bool,
    /// PD protocol decoder
    pd_decoder: PdDecoder,
    /// Decoded PD log entries
    pd_log: VecDeque<DecodedPdEntry>,
    /// Max PD log entries
    max_pd_entries: usize,
    /// Current PD status
    pd_status: Option<PdStatus>,
    /// Debounced phone connection state
    pd_connection: PdConnectionTracker,
    /// Auto-scroll PD log
    pd_auto_scroll: bool,
    /// PD panel visible
    pd_panel_visible: bool,
    /// Show decoded wire-protocol events in the shared timeline
    pd_protocol_visible: bool,
    /// Whether the USB task should drain the firmware PD trace queues
    pd_trace_enabled: bool,
    /// Firmware PD trace entries
    pd_trace_log: VecDeque<PdTraceEntry>,
    /// Max firmware PD trace entries
    max_pd_trace_entries: usize,
    /// Whether to perform USB reset on connect
    usb_reset: bool,
    /// Recording-scoped idle-sleep assertion. The display may still lock or
    /// turn off; only automatic system sleep is inhibited.
    sleep_protection_enabled: bool,
    sleep_assertion: Option<IdleSleepAssertion>,
}

struct FinalizingSegment {
    index: u32,
    recorder: Recorder,
}

impl PowerMonitorApp {
    #[allow(dead_code)]
    fn new(usb_receiver: mpsc::UnboundedReceiver<UsbMessage>, cmd_sender: mpsc::UnboundedSender<UsbCommand>) -> Self {
        Self::with_defaults(usb_receiver, cmd_sender, false)
    }

    fn new_with_context(
        cc: &eframe::CreationContext<'_>,
        usb_receiver: mpsc::UnboundedReceiver<UsbMessage>,
        cmd_sender: mpsc::UnboundedSender<UsbCommand>,
        demo_mode: bool,
    ) -> Self {
        theme::apply(&cc.egui_ctx);
        let mut app = Self::with_defaults(usb_receiver, cmd_sender, demo_mode);
        if let Some(storage) = cc.storage
            && let Some(prefs) = eframe::get_value::<AppPreferences>(storage, eframe::APP_KEY)
        {
            app.apply_preferences(prefs);
        }
        if demo_mode {
            app.enable_demo_device();
        } else {
            app.request_connect();
        }
        app
    }

    fn with_defaults(
        usb_receiver: mpsc::UnboundedReceiver<UsbMessage>,
        cmd_sender: mpsc::UnboundedSender<UsbCommand>,
        demo_mode: bool,
    ) -> Self {
        let now = Instant::now();
        Self {
            language: Language::SimplifiedChinese,
            data_points: VecDeque::new(),
            navigator_history: NavigatorHistory::default(),
            measurement_accumulator: MeasurementAccumulator::default(),
            measurement_resume_offsets: None,
            usb_receiver,
            cmd_sender,
            device_state: None,
            status: if demo_mode {
                "演示数据".to_string()
            } else {
                "搜索设备".to_string()
            },
            phase: if demo_mode {
                ConnectionPhase::Streaming
            } else {
                ConnectionPhase::Searching
            },
            next_retry_at: now,
            last_connection_error: None,
            disconnect_requested: false,
            disconnect_confirmation: false,
            clear_data_confirmation: false,
            demo_mode,
            demo_started: now,
            demo_last_tick: now,
            demo_sequence: 0,
            streaming: false,
            current_rate: SampleRateOption::Sps50,
            selected_rate: SampleRateOption::Sps50,
            time_window: TimeWindow::Sec30,
            max_points: 100000, // Safety cap for memory
            total_samples: 0,
            dropped_samples: 0,
            discarded_sequence_samples: 0,
            current_voltage: 0.0,
            current_current: 0.0,
            current_power: 0.0,
            recording_statistics: RecordingSessionStatistics::default(),
            plot_metrics: [PlotMetric::Voltage, PlotMetric::Current, PlotMetric::Power],
            active_tab: WorkspaceTab::Monitor,
            settings_open: false,
            settings_page: SettingsPage::General,
            advanced_analysis_open: false,
            visible_series: [true; 3],
            chart_follow_mode: ChartFollowMode::LatestWindow,
            display_filter: DisplayFilter::Median5,
            chart_viewport: ChartViewport::default(),
            live_plot_origin_seconds: 0.0,
            live_plot_origin_sample_index: None,
            recording_format: RecordingFormat::Parquet,
            recorder: None,
            finalizing_segments: Vec::new(),
            recording_session_directory: None,
            recording_manifest: None,
            active_segment_start: RecordingOffsets::default(),
            recording_continuation: RecordingOffsets::default(),
            next_segment_index: 0,
            recording_status: "尚未录制".to_string(),
            last_recording: None,
            recording_session_metadata: None,
            last_recording_metadata: None,
            recording_phase: RecordingPhase::Idle,
            pending_save_destination: None,
            pause_intervals: Vec::new(),
            active_pause_started_at: None,
            recording_session: false,
            recording_paused: false,
            recording_started_at: None,
            recording_elapsed_before_pause: Duration::ZERO,
            recording_energy_origin_uwh: 0.0,
            recording_energy_completed_uwh: 0.0,
            recording_total_energy_uwh: 0.0,
            recording_capacity_origin_uah: 0.0,
            recording_capacity_completed_uah: 0.0,
            recording_total_capacity_uah: 0.0,
            recording_net_energy_origin_uwh: 0.0,
            recording_net_energy_completed_uwh: 0.0,
            recording_net_energy_uwh: 0.0,
            last_recording_duration: None,
            last_recording_energy_uwh: None,
            last_recording_capacity_uah: None,
            last_recording_net_energy_uwh: None,
            auto_pause_enabled: false,
            auto_capture_metric: AutoCaptureMetric::Power,
            auto_pause_threshold_mw: 100,
            auto_pause_delay_ms: 3_000,
            auto_pause_below_since_us: None,
            auto_resume_above_since_us: None,
            pause_reason: None,
            offline_catalog: Vec::new(),
            offline_selected: None,
            offline_view: None,
            offline_device_metadata: None,
            offline_busy: false,
            offline_status: "尚未加载设备离线记录".to_string(),
            offline_export: None,
            imported_recording: None,
            recording_import: None,
            import_status: "尚未导入桌面录制".to_string(),
            plot_source: PlotSource::Live,
            cursor_readout: None,
            cursor_pinned: false,
            reset_plots_requested: false,
            pd_decoder: PdDecoder::new(),
            pd_log: VecDeque::new(),
            max_pd_entries: 1000,
            pd_status: None,
            pd_connection: PdConnectionTracker::default(),
            pd_auto_scroll: true,
            pd_panel_visible: true,
            pd_protocol_visible: true,
            pd_trace_enabled: false,
            pd_trace_log: VecDeque::new(),
            max_pd_trace_entries: 2000,
            usb_reset: false,
            sleep_protection_enabled: true,
            sleep_assertion: None,
        }
    }

    fn apply_preferences(&mut self, prefs: AppPreferences) {
        self.language = prefs.language;
        self.selected_rate = prefs.selected_rate;
        self.current_rate = prefs.selected_rate;
        self.time_window = prefs.time_window;
        self.plot_metrics = prefs.plot_metrics;
        self.recording_format = prefs.recording_format;
        self.pd_auto_scroll = prefs.pd_auto_scroll;
        self.pd_panel_visible = prefs.pd_panel_visible;
        self.pd_protocol_visible = prefs.pd_protocol_visible;
        self.pd_trace_enabled = prefs.pd_trace_enabled;
        self.usb_reset = prefs.usb_reset;
        self.sleep_protection_enabled = prefs.sleep_protection_enabled;
        self.auto_pause_enabled = prefs.auto_pause_enabled;
        self.auto_capture_metric = prefs.auto_capture_metric;
        self.auto_pause_threshold_mw = prefs.auto_pause_threshold_mw;
        self.auto_pause_delay_ms = prefs.auto_pause_delay_ms;
        self.active_tab = prefs.active_tab;
        self.visible_series = prefs.visible_series;
        if !self.visible_series.iter().any(|visible| *visible) {
            self.visible_series = [true; 3];
        }
        self.chart_follow_mode = if prefs.follow_latest && self.time_window == TimeWindow::All {
            ChartFollowMode::FullSession
        } else if prefs.follow_latest {
            ChartFollowMode::LatestWindow
        } else {
            ChartFollowMode::Manual
        };
        self.display_filter = prefs.display_filter;
    }

    fn preferences(&self) -> AppPreferences {
        AppPreferences {
            language: self.language,
            selected_rate: self.selected_rate,
            time_window: self.time_window,
            plot_metrics: self.plot_metrics,
            recording_format: self.recording_format,
            pd_auto_scroll: self.pd_auto_scroll,
            pd_panel_visible: self.pd_panel_visible,
            pd_protocol_visible: self.pd_protocol_visible,
            pd_trace_enabled: self.pd_trace_enabled,
            usb_reset: self.usb_reset,
            sleep_protection_enabled: self.sleep_protection_enabled,
            auto_pause_enabled: self.auto_pause_enabled,
            auto_capture_metric: self.auto_capture_metric,
            auto_pause_threshold_mw: self.auto_pause_threshold_mw,
            auto_pause_delay_ms: self.auto_pause_delay_ms,
            active_tab: self.active_tab,
            visible_series: self.visible_series,
            follow_latest: self.chart_follow_mode.is_following(),
            display_filter: self.display_filter,
        }
    }

    fn request_connect(&mut self) {
        if self.demo_mode || self.streaming || self.phase == ConnectionPhase::Connecting {
            return;
        }
        self.phase = ConnectionPhase::Connecting;
        self.disconnect_requested = false;
        self.status = ConnectionPhase::Connecting.label().to_string();
        self.next_retry_at = Instant::now() + Duration::from_secs(3);
        if self
            .cmd_sender
            .send(UsbCommand::Connect(self.selected_rate.to_graph_rate(), self.usb_reset))
            .is_err()
        {
            self.phase = ConnectionPhase::ConnectionError;
            self.status = "USB 任务不可用".to_string();
        }
    }

    fn maybe_retry_connection(&mut self) {
        if !self.demo_mode && self.phase == ConnectionPhase::NoDevice && Instant::now() >= self.next_retry_at {
            self.request_connect();
        }
    }

    fn enable_demo_device(&mut self) {
        let info = km003c_lib::DeviceInfo {
            model: "KM003C-DEMO".to_string(),
            hw_version: "2.1-demo".to_string(),
            mfg_date: "2026.08.29".to_string(),
            fw_version: "1.9.9-demo".to_string(),
            fw_date: "2026.08.29".to_string(),
            serial_id: "DEMO-0001".to_string(),
            uuid: "演示数据，不是实际设备".to_string(),
        };
        self.device_state = Some(Arc::new(DeviceState {
            info,
            hardware_id: km003c_lib::HardwareId::from_bytes(*b"DEMO00\x00\x00\x01\x00\x00\x00"),
            auth_level: 2,
            adcqueue_enabled: true,
        }));
        self.streaming = true;
        self.phase = ConnectionPhase::Streaming;
        self.status = "演示数据 · 实时采集模拟中".to_string();
        self.pd_connection.observe_event(true, Instant::now());
    }

    fn process_messages(&mut self) -> bool {
        let mut processed_messages = 0;
        let mut pending_measurements = Vec::new();
        while processed_messages < MAX_USB_MESSAGES_PER_FRAME {
            let Ok(msg) = self.usb_receiver.try_recv() else {
                break;
            };
            processed_messages += 1;

            // Preserve ordering around connection/rate events while avoiding
            // one writer-channel message per USB transfer during catch-up.
            if !matches!(&msg, UsbMessage::Samples(_)) && !pending_measurements.is_empty() {
                self.append_measurements(&pending_measurements);
                pending_measurements.clear();
            }
            match msg {
                UsbMessage::Connected(state) => {
                    self.phase = ConnectionPhase::Streaming;
                    self.disconnect_requested = false;
                    self.status = format!("已连接 · {}", state.model());
                    if self.recording_phase == RecordingPhase::WaitingForReconnect {
                        let expected_serial = self
                            .recording_session_metadata
                            .as_ref()
                            .map(|metadata| metadata.device.serial.as_str())
                            .unwrap_or_default();
                        if expected_serial.is_empty() || expected_serial == state.info.serial_id {
                            self.recording_phase = RecordingPhase::Recovering;
                            self.status = self
                                .language
                                .pick(
                                    "已找到原设备 · 正在恢复采样",
                                    "Original device found · Restoring sampling",
                                )
                                .to_string();
                        } else {
                            self.status = format!(
                                "{}：{}",
                                self.language.pick(
                                    "已连接另一台 KM003C，录制仍等待原序列号",
                                    "A different KM003C is connected; the recording is still waiting for the original serial number",
                                ),
                                expected_serial,
                            );
                        }
                    }
                    self.last_connection_error = None;
                    self.device_state = Some(state);
                    self.offline_catalog.clear();
                    self.offline_selected = None;
                    self.offline_status = "尚未加载设备离线记录".to_string();
                    self.pd_decoder.reset();
                    self.pd_connection = PdConnectionTracker::default();
                    if self.pd_trace_enabled {
                        let _ = self.cmd_sender.send(UsbCommand::SetPdTraceEnabled(true));
                    }
                }
                UsbMessage::ConnectionFailed(err) => {
                    let (phase, message) = i18n::connection_error(self.language, &err);
                    self.phase = phase;
                    self.status = message;
                    self.last_connection_error = Some(err.clone());
                    if phase == ConnectionPhase::NoDevice {
                        self.next_retry_at = Instant::now() + Duration::from_secs(3);
                    }
                    warn!("{}", i18n::original_error_context(self.language, &err));
                }
                UsbMessage::Samples(samples) => {
                    let rate = self.current_rate.to_graph_rate();
                    pending_measurements.extend(
                        samples
                            .into_iter()
                            .filter_map(|sample| self.measurement_accumulator.push(sample, rate))
                            .map(|sample| {
                                self.measurement_resume_offsets
                                    .map_or(sample, |offsets| apply_recording_offsets(sample, offsets))
                            }),
                    );
                }
                UsbMessage::StreamingStarted(rate) => {
                    self.streaming = true;
                    self.phase = ConnectionPhase::Streaming;
                    self.current_rate = SampleRateOption::from_graph_rate(rate);
                    self.selected_rate = self.current_rate;
                    self.status = format!("设备采样中 · {}", self.current_rate.label());
                    // A rate change starts a new continuity segment without
                    // inventing an interval across StopGraph/StartGraph.
                    self.measurement_accumulator.reset_continuity();
                    if self.recording_phase == RecordingPhase::Recovering
                        && self.pause_reason == Some(PauseReason::UsbDisconnected)
                    {
                        self.resume_recording();
                        self.recording_status = self
                            .language
                            .pick(
                                "USB 已重连 · 正在续录同一段记录",
                                "USB reconnected · Continuing the same recording",
                            )
                            .to_string();
                    }
                }
                UsbMessage::PdEvents(events) => {
                    for event in &events {
                        match &event.data {
                            PdEventData::Connect(()) => {
                                self.pd_connection.observe_event(true, std::time::Instant::now());
                            }
                            PdEventData::Disconnect(()) => {
                                self.pd_connection.observe_event(false, std::time::Instant::now());
                            }
                            PdEventData::PdMessage { .. } => {}
                        }

                        let entries = self.pd_decoder.decode_event(event);
                        for entry in entries {
                            self.pd_log.push_back(entry);
                            while self.pd_log.len() > self.max_pd_entries {
                                self.pd_log.pop_front();
                            }
                        }
                    }
                }
                UsbMessage::PdStatusUpdate(status) => {
                    self.pd_connection.observe_status(&status, std::time::Instant::now());
                    self.pd_status = Some(status);
                }
                UsbMessage::PdTrace(trace) => {
                    for entry in decode_trace(&trace) {
                        self.pd_trace_log.push_back(entry);
                        while self.pd_trace_log.len() > self.max_pd_trace_entries {
                            self.pd_trace_log.pop_front();
                        }
                    }
                }
                UsbMessage::OfflineCatalog(catalog) => {
                    self.offline_busy = false;
                    self.offline_status = if catalog.is_empty() {
                        "设备中没有离线记录".to_string()
                    } else {
                        format!("已加载 {} 条离线记录", catalog.len())
                    };
                    self.offline_selected = (!catalog.is_empty()).then_some(0);
                    self.offline_catalog = catalog;
                }
                UsbMessage::OfflineLogDownloaded(log) => {
                    let samples = log.samples.len();
                    let filename = log.metadata.filename_lossy().into_owned();
                    self.offline_view = Some(Arc::new(OfflineRecordingView::new(log)));
                    self.offline_device_metadata = self.device_state.as_ref().map(|state| RecordingMetadata {
                        model: state.info.model.clone(),
                        firmware: state.info.fw_version.clone(),
                        serial: state.info.serial_id.clone(),
                    });
                    self.offline_busy = false;
                    self.offline_status = format!("已下载 {samples} 个采样点：{filename}");
                    self.plot_source = PlotSource::Offline;
                    self.time_window = TimeWindow::All;
                    self.cursor_readout = None;
                    self.cursor_pinned = false;
                    self.reset_plots_requested = true;
                    self.chart_viewport.selection = None;
                    self.chart_follow_mode = ChartFollowMode::FullSession;
                    for metric in &mut self.plot_metrics {
                        if !metric.supports_offline() {
                            *metric = PlotMetric::Voltage;
                        }
                    }
                }
                UsbMessage::OfflineOperationFailed(error) => {
                    self.offline_busy = false;
                    self.offline_status = format!("离线记录操作失败：{error}");
                }
                UsbMessage::StreamingStopped => {
                    self.streaming = false;
                    if self.device_state.is_some() {
                        self.phase = ConnectionPhase::Disconnected;
                    }
                    self.status = "采集已停止".to_string();
                }
                UsbMessage::Error(err) => {
                    self.phase = ConnectionPhase::ConnectionError;
                    self.status = format!("连接错误：{err}");
                    self.last_connection_error = Some(err);
                }
                UsbMessage::Disconnected => {
                    self.status = if self.disconnect_requested {
                        "已断开".to_string()
                    } else {
                        "已断开 · 等待重新插入".to_string()
                    };
                    self.phase = if self.disconnect_requested {
                        ConnectionPhase::Disconnected
                    } else {
                        self.next_retry_at = Instant::now() + Duration::from_secs(3);
                        ConnectionPhase::NoDevice
                    };
                    self.streaming = false;
                    self.device_state = None;
                    self.pd_status = None;
                    self.pd_decoder.reset();
                    self.pd_connection = PdConnectionTracker::default();
                    self.offline_busy = false;
                    if self.recording_session && !self.disconnect_requested {
                        self.pause_recording_with_reason(PauseReason::UsbDisconnected);
                        self.recording_phase = RecordingPhase::WaitingForReconnect;
                        self.recording_status = self
                            .language
                            .pick(
                                "USB 已中断 · 保留当前录制并等待原设备重连",
                                "USB interrupted · Recording retained while waiting for the original device",
                            )
                            .to_string();
                    } else {
                        self.stop_recording();
                    }
                }
            }
        }

        if !pending_measurements.is_empty() {
            self.append_measurements(&pending_measurements);
        }

        self.pd_connection.update(Instant::now());
        self.maybe_retry_connection();
        self.poll_recording();
        self.poll_offline_export();
        self.poll_recording_import();
        processed_messages == MAX_USB_MESSAGES_PER_FRAME
    }

    fn recording_elapsed(&self) -> Duration {
        let running = self.recording_started_at.map_or(Duration::ZERO, |started_at| {
            let since_anchor = Instant::now().duration_since(started_at);
            if self.recorder.is_some() {
                // Once samples exist, the device-derived elapsed time is
                // authoritative. Interpolate by at most one sample period
                // so display sleep or a stalled USB link cannot invent
                // minutes that were never written to the recording.
                since_anchor.min(Duration::from_secs_f64(
                    1.0 / f64::from(self.current_rate.hertz().max(1)),
                ))
            } else {
                since_anchor
            }
        });
        self.recording_elapsed_before_pause + running
    }

    fn sync_recording_clock_to_samples(&mut self, elapsed_us: u64) {
        self.recording_elapsed_before_pause = self
            .recording_elapsed_before_pause
            .max(Duration::from_micros(elapsed_us));
        if self.recording_session && !self.recording_paused {
            self.recording_started_at = Some(Instant::now());
        }
    }

    fn displayed_recording_duration(&self) -> Duration {
        if self.recording_session {
            self.recording_elapsed()
        } else {
            self.last_recording_duration.unwrap_or(Duration::ZERO)
        }
    }

    fn displayed_cumulative_energy_uwh(&self) -> f64 {
        if self.recording_session {
            self.recording_total_energy_uwh
        } else {
            self.last_recording_energy_uwh.unwrap_or(0.0)
        }
    }

    fn displayed_recording_capacity_uah(&self) -> f64 {
        if self.recording_session {
            self.recording_total_capacity_uah
        } else {
            self.last_recording_capacity_uah.unwrap_or(0.0)
        }
    }

    fn displayed_recording_net_energy_uwh(&self) -> f64 {
        if self.recording_session {
            self.recording_net_energy_uwh
        } else {
            self.last_recording_net_energy_uwh.unwrap_or(0.0)
        }
    }

    fn freeze_recording_clock(&mut self) {
        self.recording_elapsed_before_pause = self.recording_elapsed_before_pause.max(self.recording_elapsed());
        self.recording_started_at = None;
        self.recording_paused = true;
    }

    fn pause_recording(&mut self) {
        self.pause_recording_with_reason(PauseReason::Manual);
    }

    fn pause_recording_with_reason(&mut self, reason: PauseReason) {
        if !self.recording_session || self.recording_paused {
            return;
        }
        if let Some(metadata) = &mut self.recording_session_metadata {
            let interval = RecordingTimeInterval {
                reason: reason.interval_reason(),
                started_at_utc: Utc::now(),
                ended_at_utc: None,
            };
            if reason == PauseReason::UsbDisconnected {
                metadata.disconnect_intervals.push(interval);
            } else {
                metadata.pause_intervals.push(interval);
            }
            metadata.refresh_durations(Utc::now());
        }
        self.pause_reason = Some(reason);
        self.freeze_recording_clock();
        self.recording_phase = RecordingPhase::Paused;
        self.active_pause_started_at = self
            .data_points
            .back()
            .map(|sample| self.live_display_time(sample.elapsed_seconds()));
        // MeasurementSample::energy_throughput_uwh is device-session
        // cumulative energy. Moving the origin here excludes the interval in
        // which the user paused the recording from the saved segment.
        if let Some(last) = self.data_points.back() {
            self.recording_energy_origin_uwh = last.energy_throughput_uwh;
            self.recording_capacity_origin_uah = last.charge_throughput_uah;
            self.recording_net_energy_origin_uwh = last.energy_uwh;
        }
        self.recording_energy_completed_uwh = self.recording_total_energy_uwh;
        self.recording_capacity_completed_uah = self.recording_total_capacity_uah;
        self.recording_net_energy_completed_uwh = self.recording_net_energy_uwh;
        self.auto_pause_below_since_us = None;
        self.auto_resume_above_since_us = None;
        let session_state = if reason == PauseReason::UsbDisconnected {
            SessionState::WaitingForReconnect
        } else {
            SessionState::Paused
        };
        if let Err(error) = self.seal_active_recording_segment(session_state) {
            self.recording_phase = RecordingPhase::Interrupted;
            self.recording_status = error;
            return;
        }
        self.recording_status = format!(
            "已暂停录制 · 时长 {} · 累计能量 {}",
            format_recording_duration(self.recording_elapsed()),
            format_cumulative_energy(self.recording_total_energy_uwh),
        );
    }

    fn interrupt_recording_after_writer_failure(&mut self, error: String) {
        self.freeze_recording_clock();
        self.recording_phase = RecordingPhase::Interrupted;
        self.active_pause_started_at = self
            .data_points
            .back()
            .map(|sample| self.live_display_time(sample.elapsed_seconds()));
        if let Some(last) = self.data_points.back() {
            self.recording_energy_origin_uwh = last.energy_throughput_uwh;
            self.recording_capacity_origin_uah = last.charge_throughput_uah;
            self.recording_net_energy_origin_uwh = last.energy_uwh;
        }
        self.recording_energy_completed_uwh = self.recording_total_energy_uwh;
        self.recording_capacity_completed_uah = self.recording_total_capacity_uah;
        self.recording_net_energy_completed_uwh = self.recording_net_energy_uwh;

        let seal_error = self.seal_active_recording_segment(SessionState::Interrupted).err();
        self.recording_status = match seal_error {
            Some(seal_error) => format!(
                "{}: {error}; {}: {seal_error}",
                self.language.pick("录制写入中断", "Recording writer interrupted"),
                self.language.pick("数据段封口失败", "Failed to seal the data segment"),
            ),
            None => format!(
                "{}: {error} · {}",
                self.language.pick("录制写入中断", "Recording writer interrupted"),
                self.language
                    .pick("已保留数据，可继续记录", "Data retained; recording can be resumed"),
            ),
        };
        error!("{}", self.recording_status);
    }

    fn resume_recording(&mut self) {
        let can_resume = self.recording_session
            && self.recording_paused
            && self.streaming
            && self.recorder.as_ref().is_none_or(|recorder| !recorder.is_finishing());
        if !can_resume {
            return;
        }
        let pause_reason = self.pause_reason;
        if let Some(metadata) = &mut self.recording_session_metadata {
            let intervals = if pause_reason == Some(PauseReason::UsbDisconnected) {
                &mut metadata.disconnect_intervals
            } else {
                &mut metadata.pause_intervals
            };
            if let Some(interval) = intervals
                .iter_mut()
                .rev()
                .find(|interval| interval.ended_at_utc.is_none())
            {
                interval.ended_at_utc = Some(Utc::now());
            }
            metadata.refresh_durations(Utc::now());
        }
        if self.recorder.is_none()
            && let Err(error) =
                self.start_next_recording_segment(self.data_points.back().copied(), self.recording_continuation)
        {
            self.recording_status = error;
            self.recording_phase = RecordingPhase::Interrupted;
            return;
        }
        let mut sleep_warning = None;
        if self.sleep_protection_enabled && self.sleep_assertion.is_none() {
            match IdleSleepAssertion::acquire() {
                Ok(assertion) => self.sleep_assertion = Some(assertion),
                Err(error) => {
                    sleep_warning = Some(format!(
                        "{} · {error}",
                        self.language.pick(
                            "已继续录制，但睡眠保护不可用",
                            "Recording resumed, but sleep protection is unavailable",
                        )
                    ));
                }
            }
        }
        self.rebase_recording_origins_at_latest_sample();
        if let (Some(start_seconds), Some(end_seconds)) = (
            self.active_pause_started_at.take(),
            self.data_points
                .back()
                .map(|sample| self.live_display_time(sample.elapsed_seconds())),
        ) && end_seconds >= start_seconds
        {
            self.pause_intervals.push(PauseInterval {
                start_seconds,
                end_seconds,
            });
        }
        self.recording_started_at = Some(Instant::now());
        self.recording_paused = false;
        self.pause_reason = None;
        self.recording_phase = RecordingPhase::Recording;
        if let Some(manifest) = &mut self.recording_manifest {
            manifest.state = SessionState::Recording;
        }
        if let Err(error) = self.persist_recording_manifest() {
            self.freeze_recording_clock();
            self.recording_phase = RecordingPhase::Interrupted;
            self.recording_status = error;
            return;
        }
        self.auto_pause_below_since_us = None;
        self.auto_resume_above_since_us = None;
        self.recording_status = sleep_warning.unwrap_or_else(|| {
            self.language
                .pick("录制中 · 已继续采集", "Recording · Capture resumed")
                .to_string()
        });
    }

    fn rebase_recording_origins_at_latest_sample(&mut self) {
        if let Some(last) = self.data_points.back() {
            // Resume origins must be the newest paused sample. Otherwise the
            // next delta would accidentally include energy and capacity that
            // accumulated while the file writer was paused.
            self.recording_energy_origin_uwh = last.energy_throughput_uwh;
            self.recording_capacity_origin_uah = last.charge_throughput_uah;
            self.recording_net_energy_origin_uwh = last.energy_uwh;
        }
    }

    fn finish_recording_session(&mut self) {
        if self.recording_session {
            self.last_recording_duration = Some(self.recording_elapsed());
            self.last_recording_energy_uwh = Some(self.recording_total_energy_uwh.max(0.0));
            self.last_recording_capacity_uah = Some(self.recording_total_capacity_uah.max(0.0));
            self.last_recording_net_energy_uwh = Some(self.recording_net_energy_uwh);
        }
        self.recording_session = false;
        self.recording_paused = false;
        self.recording_started_at = None;
        self.recording_elapsed_before_pause = Duration::ZERO;
        self.recording_energy_origin_uwh = 0.0;
        self.recording_energy_completed_uwh = 0.0;
        self.recording_total_energy_uwh = 0.0;
        self.recording_capacity_origin_uah = 0.0;
        self.recording_capacity_completed_uah = 0.0;
        self.recording_total_capacity_uah = 0.0;
        self.recording_net_energy_origin_uwh = 0.0;
        self.recording_net_energy_completed_uwh = 0.0;
        self.recording_net_energy_uwh = 0.0;
        self.auto_pause_below_since_us = None;
        self.auto_resume_above_since_us = None;
        self.pause_reason = None;
        self.active_pause_started_at = None;
        self.measurement_resume_offsets = None;
        if let Some(mut assertion) = self.sleep_assertion.take() {
            assertion.release();
        }
        if let Some(metadata) = self.recording_session_metadata.take() {
            self.last_recording_metadata = Some(metadata);
        }
    }

    fn cursor_readout_at(&self, time_seconds: f64) -> Option<CursorReadout> {
        let target = if time_seconds.is_finite() {
            time_seconds.max(0.0)
        } else {
            0.0
        };
        match self.plot_source {
            PlotSource::Live => {
                let absolute_target = self.live_absolute_time(target);
                let detailed_start = self
                    .data_points
                    .front()
                    .map_or(f64::INFINITY, |sample| sample.elapsed_seconds());
                if absolute_target < detailed_start {
                    self.navigator_history
                        .readout_at(absolute_target)
                        .filter(|readout| readout.time_seconds + f64::EPSILON >= self.live_plot_origin_seconds)
                        .map(|mut readout| {
                            readout.time_seconds = self.live_display_time(readout.time_seconds);
                            readout
                        })
                } else {
                    nearest_time_index(self.data_points.len(), absolute_target, |index| {
                        self.data_points[index].elapsed_seconds()
                    })
                    .map(|index| self.data_points[index])
                    .filter(|sample| sample.elapsed_seconds() + f64::EPSILON >= self.live_plot_origin_seconds)
                    .map(|sample| CursorReadout {
                        time_seconds: self.live_display_time(sample.elapsed_seconds()),
                        voltage: sample.vbus_uv as f64 / 1_000_000.0,
                        current: (sample.ibus_ua as f64 / 1_000_000.0).abs(),
                        power: (sample.power_uw as f64 / 1_000_000.0).abs(),
                        approximate: false,
                    })
                }
            }
            PlotSource::Offline => self
                .offline_view
                .as_ref()
                .and_then(|view| {
                    nearest_time_index(view.samples.len(), target, |index| {
                        view.samples[index].elapsed_seconds()
                    })
                    .map(|index| view.samples[index])
                })
                .map(|sample| CursorReadout {
                    time_seconds: sample.elapsed_seconds(),
                    voltage: sample.vbus_uv as f64 / 1_000_000.0,
                    current: (sample.ibus_ua as f64 / 1_000_000.0).abs(),
                    power: (sample.power_uw as f64 / 1_000_000.0).abs(),
                    approximate: false,
                }),
            PlotSource::Imported => self
                .imported_recording
                .as_ref()
                .and_then(|recording| {
                    nearest_time_index(recording.samples.len(), target, |index| {
                        recording.samples[index].elapsed_seconds()
                    })
                    .map(|index| recording.samples[index])
                })
                .map(|sample| CursorReadout {
                    time_seconds: sample.elapsed_seconds(),
                    voltage: sample.vbus_uv as f64 / 1_000_000.0,
                    current: (sample.ibus_ua as f64 / 1_000_000.0).abs(),
                    power: (sample.power_uw as f64 / 1_000_000.0).abs(),
                    approximate: false,
                }),
        }
    }

    fn accumulated_readout(&self) -> Option<AccumulatedReadout> {
        match self.plot_source {
            PlotSource::Live => self.data_points.back().map(|sample| AccumulatedReadout {
                cumulative_energy_uwh: sample.energy_throughput_uwh,
                capacity_uah: sample.charge_throughput_uah,
                net_energy_uwh: sample.energy_uwh,
            }),
            PlotSource::Offline => self
                .offline_view
                .as_ref()
                .and_then(|view| view.samples.last())
                .map(|sample| AccumulatedReadout {
                    cumulative_energy_uwh: sample.energy_throughput_uwh,
                    capacity_uah: sample.charge_throughput_uah,
                    net_energy_uwh: sample.energy_uwh,
                }),
            PlotSource::Imported => self
                .imported_recording
                .as_ref()
                .and_then(|recording| recording.samples.last())
                .map(|sample| AccumulatedReadout {
                    cumulative_energy_uwh: sample.energy_throughput_uwh,
                    capacity_uah: sample.charge_throughput_uah,
                    net_energy_uwh: sample.energy_uwh,
                }),
        }
    }

    fn monitor_chart_visible(&self) -> bool {
        match self.plot_source {
            // Live samples are always reflected by the large cards. The
            // waveform is a recording workspace and appears only after the
            // user deliberately starts a capture.
            PlotSource::Live => self.recording_session,
            PlotSource::Offline | PlotSource::Imported => self.source_sample_count() > 0,
        }
    }

    fn full_scope_statistics(&self) -> ScopeStatistics {
        match self.plot_source {
            PlotSource::Live => ScopeStatistics {
                duration_seconds: self.displayed_recording_duration().as_secs_f64(),
                cumulative_energy_uwh: self.displayed_cumulative_energy_uwh(),
                capacity_uah: self.displayed_recording_capacity_uah(),
                points: self.recording_session_metadata.as_ref().map_or_else(
                    || self.recorder.as_ref().map_or(0, |recorder| recorder.rows),
                    |metadata| metadata.rows,
                ),
                approximate: false,
            },
            PlotSource::Offline => self
                .offline_view
                .as_ref()
                .map_or_else(ScopeStatistics::default, |view| {
                    calculate_scope_statistics(
                        view.samples.iter().map(|sample| (sample.elapsed_seconds(), sample)),
                        NavigatorSelection {
                            start_seconds: 0.0,
                            end_seconds: self.source_end_time(),
                        },
                        &[],
                    )
                }),
            PlotSource::Imported => {
                self.imported_recording
                    .as_ref()
                    .map_or_else(ScopeStatistics::default, |recording| {
                        calculate_scope_statistics(
                            recording
                                .samples
                                .iter()
                                .map(|sample| (sample.elapsed_seconds(), sample)),
                            NavigatorSelection {
                                start_seconds: 0.0,
                                end_seconds: self.source_end_time(),
                            },
                            &[],
                        )
                    })
            }
        }
    }

    fn window_scope_statistics(&self, selection: NavigatorSelection) -> ScopeStatistics {
        if self.chart_follow_mode == ChartFollowMode::FullSession
            || (selection.start_seconds <= f64::EPSILON
                && selection.end_seconds + f64::EPSILON >= self.source_end_time())
        {
            return self.full_scope_statistics();
        }

        match self.plot_source {
            PlotSource::Live => {
                let mut excluded = self.pause_intervals.clone();
                if let Some(start_seconds) = self.active_pause_started_at {
                    excluded.push(PauseInterval {
                        start_seconds,
                        end_seconds: self.source_end_time(),
                    });
                }
                let mut result = calculate_scope_statistics(
                    self.data_points
                        .iter()
                        .filter(|sample| Some(sample.sample_index) != self.live_plot_origin_sample_index)
                        .filter(|sample| sample.elapsed_seconds() + f64::EPSILON >= self.live_plot_origin_seconds)
                        .map(|sample| (self.live_display_time(sample.elapsed_seconds()), sample)),
                    selection,
                    &excluded,
                );
                let retained_start = self
                    .data_points
                    .iter()
                    .find(|sample| Some(sample.sample_index) != self.live_plot_origin_sample_index)
                    .map_or(f64::INFINITY, |sample| self.live_display_time(sample.elapsed_seconds()));
                result.approximate = retained_start > selection.start_seconds + f64::EPSILON;
                result
            }
            PlotSource::Offline => self
                .offline_view
                .as_ref()
                .map_or_else(ScopeStatistics::default, |view| {
                    calculate_scope_statistics(
                        view.samples.iter().map(|sample| (sample.elapsed_seconds(), sample)),
                        selection,
                        &[],
                    )
                }),
            PlotSource::Imported => {
                self.imported_recording
                    .as_ref()
                    .map_or_else(ScopeStatistics::default, |recording| {
                        calculate_scope_statistics(
                            recording
                                .samples
                                .iter()
                                .map(|sample| (sample.elapsed_seconds(), sample)),
                            selection,
                            &[],
                        )
                    })
            }
        }
    }

    fn show_cursor_readout_strip(&self, ui: &mut egui::Ui, readout: Option<CursorReadout>) {
        let language = self.language;
        egui::Frame::NONE
            .fill(theme::PANEL_RAISED)
            .stroke(egui::Stroke::NONE)
            .corner_radius(egui::CornerRadius::same(6))
            .inner_margin(egui::Margin::symmetric(10, 5))
            .show(ui, |ui| {
                ui.set_min_height(22.0);
                if let Some(readout) = readout {
                    let voltage = EngineeringPresentation::for_value(readout.voltage, MeasurementUnit::Voltage);
                    let current = EngineeringPresentation::for_value(readout.current, MeasurementUnit::Current);
                    let power = EngineeringPresentation::for_value(readout.power, MeasurementUnit::Power);
                    ui.columns(4, |columns| {
                        let time_label = if readout.approximate {
                            language.pick("约值", "Approx.")
                        } else {
                            language.pick("游标", "Cursor")
                        };
                        columns[0].label(
                            egui::RichText::new(format!("{time_label}  {}", format_plot_time(readout.time_seconds)))
                                .monospace()
                                .strong()
                                .color(theme::TEXT_PRIMARY),
                        );
                        for (column, label, value, presentation, color) in [
                            (
                                1,
                                language.pick("电压", "Voltage"),
                                readout.voltage,
                                voltage,
                                theme::VOLTAGE,
                            ),
                            (
                                2,
                                language.pick("电流", "Current"),
                                readout.current,
                                current,
                                theme::CURRENT,
                            ),
                            (3, language.pick("功率", "Power"), readout.power, power, theme::POWER),
                        ] {
                            columns[column].colored_label(
                                color,
                                egui::RichText::new(format!(
                                    "{label}  {} {}",
                                    presentation.format_value(value),
                                    presentation.symbol
                                ))
                                .monospace()
                                .strong(),
                            );
                        }
                    });
                } else {
                    ui.label(
                        egui::RichText::new(language.pick(
                            "移动鼠标到曲线上查看同一时刻的电压、电流和功率",
                            "Move over the trace to inspect voltage, current, and power at the same time",
                        ))
                        .small()
                        .color(theme::TEXT_MUTED),
                    );
                }
            });
    }

    fn append_measurements(&mut self, measurements: &[MeasurementSample]) {
        self.discarded_sequence_samples = self.measurement_accumulator.cumulative_discarded_sequence_samples();
        let mut should_auto_pause = false;
        let mut should_auto_resume = false;
        let auto_rule = AutoCaptureRule {
            enabled: self.auto_pause_enabled,
            metric: self.auto_capture_metric,
            threshold_milli: self.auto_pause_threshold_mw,
            sustain_ms: self.auto_pause_delay_ms,
        };
        let threshold_micro = u64::from(auto_rule.threshold_milli).saturating_mul(1_000);
        let recovery_threshold_micro = threshold_micro.saturating_add(auto_capture_hysteresis_micro(auto_rule));
        for measurement in measurements {
            self.navigator_history.push(*measurement);
            self.data_points.push_back(*measurement);
            if self.recording_session && !self.recording_paused {
                self.recording_statistics.push(*measurement);
            }
            self.dropped_samples = measurement.cumulative_missing_samples;
            self.current_voltage = measurement.vbus_uv as f64 / 1_000_000.0;
            self.current_current = measurement.ibus_ua as f64 / 1_000_000.0;
            self.current_power = measurement.power_uw as f64 / 1_000_000.0;
            self.total_samples += 1;
            while self.data_points.len() > self.max_points {
                self.data_points.pop_front();
            }

            if self.recording_session && !self.recording_paused && auto_rule.enabled {
                let below_threshold = auto_capture_value_micro(auto_rule.metric, *measurement) <= threshold_micro;
                if below_threshold {
                    let started_at = self.auto_pause_below_since_us.get_or_insert(measurement.elapsed_us);
                    if measurement.elapsed_us.saturating_sub(*started_at)
                        >= u64::from(auto_rule.sustain_ms).saturating_mul(1_000)
                    {
                        should_auto_pause = true;
                    }
                } else {
                    self.auto_pause_below_since_us = None;
                }
            } else if self.recording_session
                && self.recording_paused
                && self.pause_reason == Some(PauseReason::Automatic(auto_rule.metric))
                && auto_rule.enabled
            {
                let above_recovery =
                    auto_capture_value_micro(auto_rule.metric, *measurement) >= recovery_threshold_micro;
                if above_recovery {
                    let started_at = self.auto_resume_above_since_us.get_or_insert(measurement.elapsed_us);
                    if measurement.elapsed_us.saturating_sub(*started_at)
                        >= u64::from(auto_rule.sustain_ms).saturating_mul(1_000)
                    {
                        should_auto_resume = true;
                    }
                } else {
                    self.auto_resume_above_since_us = None;
                }
            }
        }
        if self.recording_session && !self.recording_paused {
            if let Some(last) = measurements.last() {
                self.recording_total_energy_uwh = self.recording_energy_completed_uwh
                    + (last.energy_throughput_uwh - self.recording_energy_origin_uwh).max(0.0);
                self.recording_total_capacity_uah = self.recording_capacity_completed_uah
                    + (last.charge_throughput_uah - self.recording_capacity_origin_uah).max(0.0);
                self.recording_net_energy_uwh =
                    self.recording_net_energy_completed_uwh + (last.energy_uwh - self.recording_net_energy_origin_uwh);
            }
            let push_error = match self.recorder.as_mut() {
                Some(recorder) => recorder.push(measurements).err(),
                None => Some(
                    self.language
                        .pick(
                            "录制器不可用，实时数据仍在采集",
                            "Recording writer unavailable; live sampling is still active",
                        )
                        .to_string(),
                ),
            };
            if let Some(error) = push_error {
                self.interrupt_recording_after_writer_failure(error);
            } else {
                if let Some(snapshot) = self.recorder.as_ref().map(Recorder::summary_snapshot) {
                    self.sync_recording_clock_to_samples(snapshot.elapsed_us);
                    if let Some(metadata) = self.recording_session_metadata.as_mut() {
                        metadata.update_from_summary(&snapshot);
                        metadata.refresh_durations(Utc::now());
                    }
                }
                if should_auto_pause {
                    self.pause_recording_with_reason(PauseReason::Automatic(auto_rule.metric));
                    self.recording_status = format!(
                        "{} · {} {:.1} s ≤ {:.3} {}",
                        self.language.pick("已自动暂停", "Auto-paused"),
                        auto_rule.metric.localized_label(self.language),
                        auto_rule.sustain_ms as f64 / 1_000.0,
                        auto_rule.threshold_milli as f64 / 1_000.0,
                        match auto_rule.metric {
                            AutoCaptureMetric::Power => "W",
                            AutoCaptureMetric::Current => "A",
                            AutoCaptureMetric::Voltage => "V",
                        },
                    );
                } else {
                    self.rotate_recording_segment_if_needed();
                }
            }
        } else if should_auto_resume {
            self.resume_recording();
            self.recording_status = format!(
                "{} · {} ≥ {:.3} {}",
                self.language.pick("已自动继续", "Automatically resumed"),
                auto_rule.metric.localized_label(self.language),
                recovery_threshold_micro as f64 / 1_000_000.0,
                match auto_rule.metric {
                    AutoCaptureMetric::Power => "W",
                    AutoCaptureMetric::Current => "A",
                    AutoCaptureMetric::Voltage => "V",
                },
            );
        }
    }

    fn update_demo_data(&mut self) {
        if !self.demo_mode {
            return;
        }
        let now = Instant::now();
        if now.duration_since(self.demo_last_tick) < Duration::from_millis(20) {
            return;
        }
        self.demo_last_tick = now;
        let elapsed_us = now.duration_since(self.demo_started).as_micros() as u64;
        let t = elapsed_us as f64 / 1_000_000.0;
        let voltage = 9.0 + 0.22 * (t * 1.2).sin() + 0.04 * (t * 11.0).sin();
        let current = 1.35 + 0.18 * (t * 0.8).cos() + 0.03 * (t * 17.0).sin();
        let power = voltage * current;
        let ibus_ua = (current * 1_000_000.0) as i64;
        let power_uw = (power * 1_000_000.0) as i64;
        let (charge_uah, energy_uwh, charge_throughput_uah, energy_throughput_uwh) = self.data_points.back().map_or(
            (
                current * t / 3_600.0 * 1_000_000.0,
                power * t / 3_600.0 * 1_000_000.0,
                current.abs() * t / 3_600.0 * 1_000_000.0,
                power.abs() * t / 3_600.0 * 1_000_000.0,
            ),
            |previous| {
                let delta_us = elapsed_us.saturating_sub(previous.elapsed_us) as f64;
                let denominator = 2.0 * 3_600_000_000.0;
                (
                    previous.charge_uah + (previous.ibus_ua + ibus_ua) as f64 * delta_us / denominator,
                    previous.energy_uwh + (previous.power_uw + power_uw) as f64 * delta_us / denominator,
                    previous.charge_throughput_uah
                        + (previous.ibus_ua.abs() + ibus_ua.abs()) as f64 * delta_us / denominator,
                    previous.energy_throughput_uwh
                        + (previous.power_uw.abs() + power_uw.abs()) as f64 * delta_us / denominator,
                )
            },
        );
        let sample = MeasurementSample {
            elapsed_us,
            sample_index: u64::from(self.demo_sequence),
            sequence: self.demo_sequence,
            marker: 0xD3,
            sample_rate_hz: 50,
            missing_samples: 0,
            gap_duration_us: 0,
            interpolated: false,
            cumulative_missing_samples: 0,
            cumulative_interpolated_duration_us: 0,
            discarded_sequence_samples: 0,
            cumulative_discarded_sequence_samples: 0,
            vbus_uv: (voltage * 1_000_000.0) as i64,
            ibus_ua,
            power_uw,
            charge_uah,
            energy_uwh,
            charge_throughput_uah,
            energy_throughput_uwh,
            cc1_uv: 620_000,
            cc2_uv: 40_000,
            dp_uv: 540_000,
            dm_uv: 510_000,
        };
        self.demo_sequence = self.demo_sequence.wrapping_add(1);
        self.append_measurements(&[sample]);
        if self.pd_log.is_empty() {
            self.pd_log.push_back(DecodedPdEntry {
                timestamp_seconds: 0.0,
                category: PdCategory::Connect,
                summary: "Type-C attached (demo)".to_string(),
                details: vec!["演示数据 · 无真实 USB 报文".to_string()],
            });
            self.pd_log.push_back(DecodedPdEntry {
                timestamp_seconds: 0.12,
                category: PdCategory::SourceCaps,
                summary: "Source_Capabilities 9V/2A".to_string(),
                details: vec!["PDO 0: 5V 3A; PDO 1: 9V 2A".to_string()],
            });
        }
    }

    fn clear_data(&mut self) {
        self.data_points.clear();
        self.navigator_history.clear();
        self.cursor_readout = None;
        self.cursor_pinned = false;
        self.reset_plots_requested = true;
        self.chart_viewport.selection = None;
        self.chart_follow_mode = ChartFollowMode::LatestWindow;
        self.live_plot_origin_seconds = 0.0;
        self.live_plot_origin_sample_index = None;
        self.total_samples = 0;
        self.dropped_samples = 0;
        self.discarded_sequence_samples = 0;
        self.recording_statistics = RecordingSessionStatistics::default();
        self.measurement_resume_offsets = None;
        self.last_recording = None;
        self.last_recording_duration = None;
        self.last_recording_energy_uwh = None;
        self.last_recording_capacity_uah = None;
        self.last_recording_net_energy_uwh = None;
        self.recording_phase = RecordingPhase::Idle;
        self.recording_status = "尚未录制".to_string();
        self.measurement_accumulator.reset();
        info!("Data cleared");
    }

    fn clear_pd_log(&mut self) {
        self.pd_log.clear();
        self.pd_trace_log.clear();
        info!("PD timeline cleared");
    }

    fn create_recording_session_storage(&mut self, metadata: RecordingSessionMetadataV1) -> Result<(), String> {
        let directory = application_recordings_directory()
            .join("Pending")
            .join(&metadata.session_id);
        std::fs::create_dir_all(directory.join("segments"))
            .map_err(|error| format!("无法创建录制会话目录 {}：{error}", directory.display()))?;
        let manifest = RecordingSessionManifestV1::new(self.recording_format, metadata.clone());
        write_manifest(&directory, &manifest)?;
        self.recording_session_directory = Some(directory);
        self.recording_manifest = Some(manifest);
        self.recording_session_metadata = Some(metadata);
        self.active_segment_start = RecordingOffsets::default();
        self.recording_continuation = RecordingOffsets::default();
        self.next_segment_index = 0;
        Ok(())
    }

    fn persist_recording_manifest(&mut self) -> Result<(), String> {
        let (Some(directory), Some(manifest)) = (&self.recording_session_directory, &mut self.recording_manifest)
        else {
            return Ok(());
        };
        if let Some(metadata) = &self.recording_session_metadata {
            manifest.metadata = metadata.clone();
        }
        write_manifest(directory, manifest).map(|_| ())
    }

    fn start_next_recording_segment(
        &mut self,
        origin: Option<MeasurementSample>,
        offsets: RecordingOffsets,
    ) -> Result<(), String> {
        if self.recorder.is_some() {
            return Ok(());
        }
        let directory = self
            .recording_session_directory
            .as_ref()
            .ok_or_else(|| "录制会话目录不存在".to_string())?;
        let manifest = self
            .recording_manifest
            .as_ref()
            .ok_or_else(|| "录制会话清单不存在".to_string())?;
        let index = self.next_segment_index;
        let file_name = format!("segment-{index:06}.{}", manifest.format.extension());
        let path = directory.join("segments").join(&file_name);
        let recorder =
            Recorder::start_with_offsets(path, manifest.format, manifest.metadata.device.clone(), origin, offsets)?;
        self.active_segment_start = offsets;
        self.recording_continuation = offsets;
        self.next_segment_index = self.next_segment_index.saturating_add(1);
        if let Some(manifest) = &mut self.recording_manifest {
            manifest.segments.push(RecordingSegmentMetadata {
                index,
                file_name,
                start_row: offsets.sample_index,
                end_row: offsets.sample_index,
                start_elapsed_us: offsets.elapsed_us,
                end_elapsed_us: offsets.elapsed_us,
                sealed: false,
            });
        }
        self.recorder = Some(recorder);
        self.persist_recording_manifest()
    }

    fn seal_active_recording_segment(&mut self, state: SessionState) -> Result<(), String> {
        let Some(mut recorder) = self.recorder.take() else {
            if let Some(manifest) = &mut self.recording_manifest {
                manifest.state = state;
            }
            return self.persist_recording_manifest();
        };
        let index = self.next_segment_index.saturating_sub(1);
        let snapshot = recorder.summary_snapshot();
        self.recording_continuation = recorder.continuation_offsets();
        recorder.request_finish()?;
        if let Some(metadata) = &mut self.recording_session_metadata {
            metadata.update_from_summary(&snapshot);
            metadata.refresh_durations(Utc::now());
        }
        if let Some(manifest) = &mut self.recording_manifest {
            manifest.state = state;
            if let Some(segment) = manifest.segments.iter_mut().find(|segment| segment.index == index) {
                segment.end_row = snapshot.rows;
                segment.end_elapsed_us = snapshot.elapsed_us;
            }
        }
        self.finalizing_segments.push(FinalizingSegment { index, recorder });
        self.persist_recording_manifest()
    }

    fn rotate_recording_segment_if_needed(&mut self) {
        let Some(recorder) = &self.recorder else {
            return;
        };
        let segment_rows = recorder.rows.saturating_sub(self.active_segment_start.sample_index);
        let segment_elapsed = recorder.elapsed_us.saturating_sub(self.active_segment_start.elapsed_us);
        if segment_rows < 32_768 && segment_elapsed < 30_000_000 {
            return;
        }
        let origin = self.data_points.back().copied();
        if let Err(error) = self.seal_active_recording_segment(SessionState::Recording) {
            self.recording_status = error;
            self.recording_phase = RecordingPhase::Interrupted;
            return;
        }
        if let Err(error) = self.start_next_recording_segment(origin, self.recording_continuation) {
            self.recording_status = error;
            self.recording_phase = RecordingPhase::Interrupted;
            self.freeze_recording_clock();
        }
    }

    fn start_recording(&mut self) {
        let Some(state) = &self.device_state else {
            self.recording_status = "请先连接 KM003C，再开始录制".to_string();
            return;
        };
        if self.recorder.is_some() || self.recording_session {
            return;
        }

        let device_metadata = RecordingMetadata {
            model: state.info.model.clone(),
            firmware: state.info.fw_version.clone(),
            serial: state.info.serial_id.clone(),
        };
        let session_metadata = RecordingSessionMetadataV1::new(Utc::now(), device_metadata, self.current_rate.hertz());
        if let Err(error) = self.create_recording_session_storage(session_metadata) {
            self.recording_status = error;
            self.recording_phase = RecordingPhase::Interrupted;
            return;
        }
        let origin_sample = self.data_points.back().copied();
        if let Err(error) = self.start_next_recording_segment(origin_sample, RecordingOffsets::default()) {
            self.recording_status = error;
            self.recording_phase = RecordingPhase::Interrupted;
            return;
        }

        self.close_imported_recording();
        self.plot_source = PlotSource::Live;
        self.live_plot_origin_seconds = origin_sample.map_or(0.0, |sample| sample.elapsed_seconds());
        self.live_plot_origin_sample_index = origin_sample.map(|sample| sample.sample_index);
        self.navigator_history.clear();
        if let Some(sample) = origin_sample {
            self.navigator_history.push(sample);
        }
        self.chart_follow_mode = ChartFollowMode::FullSession;
        self.chart_viewport.selection = None;
        self.chart_viewport.drag = None;
        self.cursor_readout = None;
        self.cursor_pinned = false;
        self.reset_plots_requested = true;
        self.recording_status = self
            .language
            .pick(
                "记录中 · 已启用分段恢复，保存时再选择目标位置",
                "Recording · Segmented recovery is active; choose the destination when saving",
            )
            .to_string();
        self.last_recording = None;
        self.last_recording_metadata = None;
        self.last_recording_duration = None;
        self.last_recording_energy_uwh = None;
        self.last_recording_capacity_uah = None;
        self.last_recording_net_energy_uwh = None;
        self.recording_statistics = RecordingSessionStatistics::default();
        self.recording_session = true;
        self.recording_paused = false;
        self.recording_phase = RecordingPhase::Recording;
        self.pending_save_destination = None;
        self.pause_intervals.clear();
        self.active_pause_started_at = None;
        self.measurement_resume_offsets = None;
        self.recording_started_at = Some(Instant::now());
        self.recording_elapsed_before_pause = Duration::ZERO;
        self.recording_energy_origin_uwh = self
            .data_points
            .back()
            .map_or(0.0, |sample| sample.energy_throughput_uwh);
        self.recording_energy_completed_uwh = 0.0;
        self.recording_total_energy_uwh = 0.0;
        self.recording_capacity_origin_uah = self
            .data_points
            .back()
            .map_or(0.0, |sample| sample.charge_throughput_uah);
        self.recording_capacity_completed_uah = 0.0;
        self.recording_total_capacity_uah = 0.0;
        self.recording_net_energy_origin_uwh = self.data_points.back().map_or(0.0, |sample| sample.energy_uwh);
        self.recording_net_energy_completed_uwh = 0.0;
        self.recording_net_energy_uwh = 0.0;
        self.auto_pause_below_since_us = None;
        self.auto_resume_above_since_us = None;
        self.pause_reason = None;
        if self.sleep_protection_enabled {
            match IdleSleepAssertion::acquire() {
                Ok(assertion) => self.sleep_assertion = Some(assertion),
                Err(error) => {
                    self.sleep_assertion = None;
                    self.recording_status = format!(
                        "{} · {error}",
                        self.language.pick(
                            "录制已开始，但睡眠保护不可用",
                            "Recording started, but sleep protection is unavailable",
                        )
                    );
                }
            }
        }
    }

    fn close_imported_recording(&mut self) {
        self.imported_recording = None;
        if self.plot_source == PlotSource::Imported {
            self.plot_source = PlotSource::Live;
        }
        self.import_status = self
            .language
            .pick("尚未导入桌面录制", "No desktop recording imported")
            .to_string();
        self.cursor_readout = None;
        self.cursor_pinned = false;
        self.chart_viewport.selection = None;
        self.chart_follow_mode = self.preferred_follow_mode();
        self.reset_plots_requested = true;
    }

    fn open_recoverable_session(&mut self, directory: PathBuf, manifest: RecordingSessionManifestV1) {
        let preview = directory.join(format!("recovery-preview.{}", manifest.format.extension()));
        match merge_session_segments(&directory, &manifest, &preview).and_then(|summary| {
            let mut metadata = manifest.metadata.clone();
            metadata.update_from_summary(&summary);
            write_sidecar(&preview, &metadata).map(|_| ())
        }) {
            Ok(()) => self.start_recording_import(preview),
            Err(error) => {
                self.import_status = format!(
                    "{}：{error}",
                    self.language
                        .pick("无法查看恢复会话", "Unable to view the recovery session")
                );
            }
        }
    }

    fn save_recoverable_session(&mut self, directory: PathBuf, mut manifest: RecordingSessionManifestV1) {
        let started = manifest.metadata.timestamps.started_at_beijing.replace([' ', ':'], "-");
        let filename = if manifest.metadata.timestamps.ended_at_utc.is_some() {
            manifest.metadata.suggested_filename(manifest.format)
        } else {
            format!(
                "KM003C_{}_recovered_{}SPS.{}",
                started.trim_end_matches(" BJT"),
                manifest.metadata.sample_rate_hz,
                manifest.format.extension(),
            )
        };
        let Some(destination) = self.select_recording_path_with_filename(
            &filename,
            self.language.pick("保存可恢复录制", "Save recoverable recording"),
        ) else {
            return;
        };
        let result = merge_session_segments(&directory, &manifest, &destination).and_then(|summary| {
            manifest.metadata.update_from_summary(&summary);
            manifest.metadata.mark_saved(Utc::now());
            write_sidecar(&destination, &manifest.metadata)?;
            {
                let restored = recording_import::load_recording(&destination)?;
                if restored.samples.len() as u64 != summary.rows {
                    return Err("恢复文件校验点数不一致".to_string());
                }
                Ok(())
            }
        });
        match result {
            Ok(()) => {
                self.import_status = format!(
                    "{}：{}",
                    self.language.pick("恢复录制已保存", "Recoverable recording saved"),
                    destination.display()
                );
                if let Err(error) = std::fs::remove_dir_all(&directory) {
                    warn!(
                        "recovered recording saved but session directory {} could not be removed: {error}",
                        directory.display()
                    );
                }
            }
            Err(error) => {
                self.import_status = format!(
                    "{}：{error}；{} {}",
                    self.language
                        .pick("恢复录制保存失败", "Failed to save recoverable recording"),
                    self.language.pick("原会话仍保留在", "The original session remains at"),
                    directory.display(),
                );
            }
        }
    }

    fn continue_recoverable_session(&mut self, directory: PathBuf, mut manifest: RecordingSessionManifestV1) {
        if self.recording_session {
            self.recording_status = self
                .language
                .pick("请先保存当前录制", "Save the current recording first")
                .to_string();
            return;
        }
        let Some(state) = &self.device_state else {
            self.recording_status = self
                .language
                .pick(
                    "请先连接原 KM003C，再继续恢复录制",
                    "Connect the original KM003C before continuing",
                )
                .to_string();
            return;
        };
        if !manifest.metadata.device.serial.is_empty() && manifest.metadata.device.serial != state.info.serial_id {
            self.recording_status = format!(
                "{}：{}",
                self.language.pick(
                    "设备序列号不匹配，需要原设备",
                    "Serial number mismatch; the original device is required"
                ),
                manifest.metadata.device.serial,
            );
            return;
        }
        let preview = directory.join(format!("recovery-continue.{}", manifest.format.extension()));
        let loaded = merge_session_segments(&directory, &manifest, &preview)
            .and_then(|summary| {
                manifest.metadata.update_from_summary(&summary);
                write_sidecar(&preview, &manifest.metadata).map(|_| ())
            })
            .and_then(|_| recording_import::load_recording(&preview));
        let recording = match loaded {
            Ok(recording) => recording,
            Err(error) => {
                self.recording_status = format!(
                    "{}：{error}",
                    self.language
                        .pick("恢复会话读取失败", "Failed to read the recovery session")
                );
                return;
            }
        };
        let Some(last) = recording.samples.last().copied() else {
            self.recording_status = self
                .language
                .pick("恢复会话没有采样点", "The recovery session has no samples")
                .to_string();
            return;
        };
        let offsets = RecordingOffsets {
            elapsed_us: last.elapsed_us,
            sample_index: last.sample_index.saturating_add(1),
            charge_uah: last.charge_uah,
            energy_uwh: last.energy_uwh,
            charge_throughput_uah: last.charge_throughput_uah,
            energy_throughput_uwh: last.energy_throughput_uwh,
            cumulative_missing_samples: last.cumulative_missing_samples,
            cumulative_interpolated_duration_us: last.cumulative_interpolated_duration_us,
            cumulative_discarded_sequence_samples: last.cumulative_discarded_sequence_samples,
        };

        self.data_points.clear();
        self.navigator_history.clear();
        for sample in recording.samples.iter().copied() {
            self.navigator_history.push(sample);
            self.data_points.push_back(sample);
            while self.data_points.len() > self.max_points {
                self.data_points.pop_front();
            }
        }
        self.current_voltage = last.vbus_uv as f64 / 1_000_000.0;
        self.current_current = last.ibus_ua as f64 / 1_000_000.0;
        self.current_power = last.power_uw as f64 / 1_000_000.0;
        self.recording_statistics = RecordingSessionStatistics::from_measurements(recording.samples.iter());
        manifest.state = SessionState::Paused;
        manifest.metadata.pause_intervals.push(RecordingTimeInterval {
            reason: IntervalReason::ApplicationRestart,
            started_at_utc: Utc::now(),
            ended_at_utc: None,
        });
        self.recording_session_metadata = Some(manifest.metadata.clone());
        self.recording_manifest = Some(manifest.clone());
        self.recording_session_directory = Some(directory);
        self.next_segment_index = manifest
            .segments
            .iter()
            .map(|segment| segment.index)
            .max()
            .map_or(0, |index| index.saturating_add(1));
        self.recording_continuation = offsets;
        self.active_segment_start = offsets;
        self.measurement_resume_offsets = Some(offsets);
        self.recording_session = true;
        self.recording_paused = true;
        self.pause_reason = Some(PauseReason::Manual);
        self.recording_phase = RecordingPhase::Paused;
        self.recording_started_at = None;
        self.recording_elapsed_before_pause = Duration::from_micros(last.elapsed_us);
        self.recording_energy_completed_uwh = last.energy_throughput_uwh;
        self.recording_total_energy_uwh = last.energy_throughput_uwh;
        self.recording_energy_origin_uwh = last.energy_throughput_uwh;
        self.recording_capacity_completed_uah = last.charge_throughput_uah;
        self.recording_total_capacity_uah = last.charge_throughput_uah;
        self.recording_capacity_origin_uah = last.charge_throughput_uah;
        self.recording_net_energy_completed_uwh = last.energy_uwh;
        self.recording_net_energy_uwh = last.energy_uwh;
        self.recording_net_energy_origin_uwh = last.energy_uwh;
        self.live_plot_origin_seconds = 0.0;
        self.live_plot_origin_sample_index = None;
        self.plot_source = PlotSource::Live;
        self.chart_follow_mode = ChartFollowMode::FullSession;
        self.chart_viewport.selection = None;
        self.active_pause_started_at = Some(last.elapsed_seconds());
        self.reset_plots_requested = true;
        self.recording_status = self
            .language
            .pick(
                "恢复会话已载入 · 点击“继续记录”后续录",
                "Recovery session loaded · Select Resume to continue",
            )
            .to_string();
        let _ = self.persist_recording_manifest();
    }

    fn save_recording(&mut self) {
        if !self.recording_session || self.recording_phase == RecordingPhase::Finalizing {
            return;
        }
        if !self.recording_paused {
            self.pause_recording();
        }
        let ended_at = Utc::now();
        let suggested_filename = self.recording_session_metadata.as_ref().map_or_else(
            || format!("KM003C-Recording.{}", self.recording_format.extension()),
            |metadata| {
                let mut preview = metadata.clone();
                preview.timestamps.set_ended(ended_at);
                preview.suggested_filename(self.recording_format)
            },
        );
        let title = self.language.pick("保存 KM003C 录制", "Save KM003C recording");
        let Some(destination) = self.select_recording_path_with_filename(&suggested_filename, title) else {
            self.recording_status = "已取消保存 · 临时录制仍保留，可继续记录或再次保存".to_string();
            return;
        };
        if let Some(metadata) = &mut self.recording_session_metadata {
            metadata.timestamps.set_ended(ended_at);
            metadata.refresh_durations(ended_at);
        }
        self.pending_save_destination = Some(destination);
        if let Some(manifest) = &mut self.recording_manifest {
            manifest.state = SessionState::Finalizing;
        }
        if let Err(error) = self.persist_recording_manifest() {
            self.recording_status = error;
            self.recording_phase = RecordingPhase::Interrupted;
            return;
        }
        self.recording_phase = RecordingPhase::Finalizing;
        self.recording_status = self
            .language
            .pick("正在合并并校验录制数据…", "Merging and validating recording data…")
            .to_string();
    }

    fn export_buffer(&mut self) {
        let Some(state) = &self.device_state else {
            self.recording_status = "请先连接 KM003C，再导出数据".to_string();
            return;
        };
        let Some(first) = self.data_points.front().copied() else {
            self.recording_status = "曲线缓冲区为空".to_string();
            return;
        };
        let metadata = RecordingMetadata {
            model: state.info.model.clone(),
            firmware: state.info.fw_version.clone(),
            serial: state.info.serial_id.clone(),
        };
        let Some(path) = self.select_recording_path("km003c-buffer", "导出 KM003C 曲线缓冲区") else {
            return;
        };

        match Recorder::start(path.clone(), self.recording_format, metadata, Some(first)) {
            Ok(mut recorder) => {
                let samples = self.data_points.iter().copied().collect::<Vec<_>>();
                match recorder.push(&samples).and_then(|()| recorder.request_finish()) {
                    Ok(()) => {
                        self.recording_status = format!("正在导出 {}", path.display());
                        self.last_recording = None;
                        self.recorder = Some(recorder);
                    }
                    Err(error) => self.recording_status = error,
                }
            }
            Err(error) => self.recording_status = error,
        }
    }

    fn select_recording_path(&self, prefix: &str, title: &str) -> Option<std::path::PathBuf> {
        let unix_seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs());
        let filename = format!("{prefix}-{unix_seconds}.{}", self.recording_format.extension());
        self.select_recording_path_with_filename(&filename, title)
    }

    fn select_recording_path_with_filename(&self, filename: &str, title: &str) -> Option<PathBuf> {
        let mut path = rfd::FileDialog::new()
            .set_title(title)
            .add_filter(self.recording_format.label(), &[self.recording_format.extension()])
            .set_file_name(filename)
            .save_file()?;
        path.set_extension(self.recording_format.extension());
        Some(path)
    }

    fn stop_recording(&mut self) {
        if self.recording_session {
            self.freeze_recording_clock();
            if let Some(metadata) = &mut self.recording_session_metadata {
                let now = Utc::now();
                metadata.timestamps.set_ended(now);
                metadata.refresh_durations(now);
            }
            if let Err(error) = self.seal_active_recording_segment(SessionState::Interrupted) {
                self.recording_status = error;
            } else {
                self.recording_status = self
                    .language
                    .pick(
                        "录制已安全封口并保留在恢复列表",
                        "Recording safely sealed and retained in the recovery list",
                    )
                    .to_string();
            }
            self.recording_phase = RecordingPhase::Interrupted;
            if let Some(mut assertion) = self.sleep_assertion.take() {
                assertion.release();
            }
            return;
        }
        let Some(recorder) = &mut self.recorder else {
            return;
        };
        if let Err(error) = recorder.request_finish() {
            self.recording_status = error;
            self.recording_phase = RecordingPhase::Interrupted;
        } else {
            self.recording_status = format!("正在安全结束录制：{}", recorder.path.display());
            if self.recording_session {
                self.recording_phase = RecordingPhase::Finalizing;
            }
        }
    }

    fn poll_finalizing_recording_segments(&mut self) {
        let mut manifest_changed = false;
        for index in (0..self.finalizing_segments.len()).rev() {
            let event = self.finalizing_segments[index].recorder.poll_event();
            let Some(event) = event else {
                continue;
            };
            let segment = self.finalizing_segments.swap_remove(index);
            match event {
                RecordingEvent::Finished(summary) | RecordingEvent::Interrupted(summary, _) => {
                    if let Some(manifest) = &mut self.recording_manifest
                        && let Some(metadata) = manifest
                            .segments
                            .iter_mut()
                            .find(|metadata| metadata.index == segment.index)
                    {
                        metadata.end_row = summary.rows;
                        metadata.end_elapsed_us = summary.elapsed_us;
                        metadata.sealed = true;
                    }
                    if let Some(metadata) = &mut self.recording_session_metadata {
                        metadata.update_from_summary(&summary);
                        metadata.refresh_durations(Utc::now());
                    }
                    manifest_changed = true;
                }
                RecordingEvent::Failed(error) => {
                    if let Some(manifest) = &mut self.recording_manifest {
                        manifest.state = SessionState::Interrupted;
                    }
                    self.recording_phase = RecordingPhase::Interrupted;
                    self.recording_status = format!(
                        "{}：{error}",
                        self.language
                            .pick("录制数据段封口失败", "Failed to seal a recording segment")
                    );
                    manifest_changed = true;
                }
            }
        }
        if manifest_changed && let Err(error) = self.persist_recording_manifest() {
            self.recording_phase = RecordingPhase::Interrupted;
            self.recording_status = error;
        }
    }

    fn finalize_saved_recording_session(&mut self) {
        if self.recording_phase != RecordingPhase::Finalizing
            || self.recorder.is_some()
            || !self.finalizing_segments.is_empty()
        {
            return;
        }
        let (Some(directory), Some(destination), Some(mut manifest), Some(mut metadata)) = (
            self.recording_session_directory.clone(),
            self.pending_save_destination.clone(),
            self.recording_manifest.clone(),
            self.recording_session_metadata.clone(),
        ) else {
            return;
        };
        let ended_at = metadata.timestamps.ended_at_utc.unwrap_or_else(Utc::now);
        for interval in metadata
            .pause_intervals
            .iter_mut()
            .chain(metadata.disconnect_intervals.iter_mut())
            .filter(|interval| interval.ended_at_utc.is_none())
        {
            interval.ended_at_utc = Some(ended_at);
        }
        let summary = RecordingSummary {
            path: destination.clone(),
            rows: metadata.rows,
            elapsed_us: metadata.effective_duration_us,
            missing_samples: metadata.missing_samples,
            interpolated_duration_us: metadata.interpolated_duration_us,
            discarded_sequence_samples: metadata.discarded_sequence_samples,
            charge_uah: metadata.net_charge_uah,
            energy_uwh: metadata.net_energy_uwh,
            charge_throughput_uah: metadata.cumulative_capacity_uah,
            energy_throughput_uwh: metadata.cumulative_energy_uwh,
        };
        metadata.finalize(&summary, ended_at);
        metadata.mark_saved(Utc::now());
        manifest.metadata = metadata.clone();
        manifest.state = SessionState::Saved;

        let result = merge_session_segments(&directory, &manifest, &destination).and_then(|summary| {
            write_sidecar(&destination, &metadata)?;
            let imported = recording_import::load_recording(&destination)?;
            if imported.samples.len() as u64 != metadata.rows {
                return Err(format!(
                    "合并校验失败：预期 {} 点，实际 {} 点",
                    metadata.rows,
                    imported.samples.len()
                ));
            }
            if imported
                .metadata
                .as_ref()
                .is_none_or(|restored| restored.session_id != metadata.session_id)
            {
                return Err("合并校验失败：会话元数据未能重新读取".to_string());
            }
            Ok(summary)
        });

        match result {
            Ok(summary) => {
                self.recording_session_metadata = Some(metadata.clone());
                self.last_recording = Some(summary.clone());
                self.last_recording_metadata = Some(metadata.clone());
                self.recording_status = format!(
                    "{} · {} → {} · {} pts · {:.3}% · {}",
                    self.language.pick("已保存", "Saved"),
                    metadata.timestamps.started_at_beijing,
                    metadata
                        .timestamps
                        .ended_at_beijing
                        .as_deref()
                        .unwrap_or(self.language.pick("结束时间未知", "End time unknown")),
                    summary.rows,
                    summary.completeness_percent(),
                    destination.display(),
                );
                self.recording_phase = RecordingPhase::Saved;
                self.pending_save_destination = None;
                self.finish_recording_session();
                self.recording_manifest = None;
                self.recording_session_directory = None;
                if let Err(error) = std::fs::remove_dir_all(&directory) {
                    warn!(
                        "recording saved but recovery directory {} could not be removed: {error}",
                        directory.display()
                    );
                }
            }
            Err(error) => {
                manifest.state = SessionState::Interrupted;
                self.recording_manifest = Some(manifest);
                let _ = self.persist_recording_manifest();
                self.recording_phase = RecordingPhase::Interrupted;
                self.recording_status = format!(
                    "{}：{error}；{} {}",
                    self.language.pick("保存失败", "Save failed"),
                    self.language.pick("恢复会话保留在", "Recovery session retained at"),
                    directory.display(),
                );
            }
        }
    }

    fn poll_recording(&mut self) {
        // `Recording` is a promise to the UI: every accepted sample must have
        // a live writer behind it. Never leave a red recording badge on screen
        // when the writer has already disappeared or is being finalized.
        // Besides making the clock lie, the old inconsistent state routed the
        // Pause button into Resume and made every recording control appear
        // frozen while the live chart kept moving.
        if self.recording_session && self.recording_phase == RecordingPhase::Recording {
            let runtime_error = if self.recording_paused {
                Some(self.language.pick(
                    "录制状态不一致：会话已暂停但界面仍显示录制中",
                    "Inconsistent recording state: the session is paused while the UI reports recording",
                ))
            } else {
                match self.recorder.as_ref() {
                    None => Some(self.language.pick(
                        "录制状态不一致：录制器已退出",
                        "Inconsistent recording state: the recording writer has exited",
                    )),
                    Some(recorder) if recorder.is_finishing() => Some(self.language.pick(
                        "录制状态不一致：录制器正在结束",
                        "Inconsistent recording state: the recording writer is finishing",
                    )),
                    Some(_) => None,
                }
            };
            if let Some(error) = runtime_error {
                self.interrupt_recording_after_writer_failure(error.to_string());
            }
        }

        // Defensive migration for the pre-fix state where a writer error set
        // `finishing` but left the recorder in the active slot. Without this,
        // manifest-backed sessions never polled its completion and the Pause
        // button appeared to do nothing forever.
        if self.recording_manifest.is_some()
            && self.recorder.as_ref().is_some_and(Recorder::is_finishing)
            && let Some(recorder) = self.recorder.take()
        {
            let index = self.next_segment_index.saturating_sub(1);
            let snapshot = recorder.summary_snapshot();
            self.recording_continuation = recorder.continuation_offsets();
            if let Some(metadata) = &mut self.recording_session_metadata {
                metadata.update_from_summary(&snapshot);
                metadata.refresh_durations(Utc::now());
            }
            if let Some(manifest) = &mut self.recording_manifest
                && let Some(segment) = manifest.segments.iter_mut().find(|segment| segment.index == index)
            {
                segment.end_row = snapshot.rows;
                segment.end_elapsed_us = snapshot.elapsed_us;
            }
            self.finalizing_segments.push(FinalizingSegment { index, recorder });
            if self.recording_session
                && matches!(self.recording_phase, RecordingPhase::Recording | RecordingPhase::Paused)
            {
                self.freeze_recording_clock();
                self.recording_phase = RecordingPhase::Interrupted;
                self.recording_status = self
                    .language
                    .pick(
                        "已恢复异常录制状态 · 可点击继续记录",
                        "Recovered an interrupted writer state · Select Resume to continue",
                    )
                    .to_string();
            }
        }
        self.poll_finalizing_recording_segments();
        if self.recording_manifest.is_some() {
            self.finalize_saved_recording_session();
            return;
        }
        let event = self.recorder.as_mut().and_then(Recorder::poll_event);
        match event {
            Some(RecordingEvent::Finished(mut summary)) => {
                let was_recording_session = self.recording_session;
                if was_recording_session && let Some(destination) = self.pending_save_destination.take() {
                    let pending_path = summary.path.clone();
                    match stage_pending_recording(&pending_path, &destination) {
                        Ok(()) => summary.path = destination,
                        Err(error) => {
                            self.recording_status = format!("{error}；恢复文件仍保留在 {}", summary.path.display());
                            self.recording_phase = RecordingPhase::Interrupted;
                            self.last_recording = Some(summary);
                            self.finish_recording_session();
                            self.recorder = None;
                            return;
                        }
                    }
                    let ended_at = self
                        .recording_session_metadata
                        .as_ref()
                        .and_then(|metadata| metadata.timestamps.ended_at_utc)
                        .unwrap_or_else(Utc::now);
                    if let Some(metadata) = &mut self.recording_session_metadata {
                        if let Some(interval) = metadata
                            .pause_intervals
                            .iter_mut()
                            .rev()
                            .find(|interval| interval.ended_at_utc.is_none())
                        {
                            interval.ended_at_utc = Some(ended_at);
                        }
                        metadata.finalize(&summary, ended_at);
                        metadata.mark_saved(Utc::now());
                        let sidecar_result = write_sidecar(&summary.path, metadata).and_then(|_| {
                            read_sidecar(&summary.path)?.ok_or_else(|| "保存后未找到 KM003C 元数据伴随文件".to_string())
                        });
                        if let Err(error) = sidecar_result {
                            self.recording_status = format!(
                                "数据文件已写入，但元数据保存失败：{error}；恢复文件仍保留在 {}",
                                pending_path.display()
                            );
                            self.recording_phase = RecordingPhase::Interrupted;
                            self.last_recording = Some(summary);
                            self.finish_recording_session();
                            self.recorder = None;
                            return;
                        }
                    }
                    if pending_path != summary.path
                        && let Err(error) = std::fs::remove_file(&pending_path)
                    {
                        warn!(
                            "saved recording but could not remove pending copy {}: {error}",
                            pending_path.display()
                        );
                    }
                }
                self.recording_status = format!(
                    "已保存 {} 个采样点到 {}（完整度 {:.4}%）",
                    summary.rows,
                    summary.path.display(),
                    summary.completeness_percent()
                );
                self.last_recording = Some(summary);
                if was_recording_session {
                    self.recording_phase = RecordingPhase::Saved;
                    self.finish_recording_session();
                }
                self.recorder = None;
            }
            Some(RecordingEvent::Interrupted(summary, reason)) => {
                self.recording_status = format!(
                    "{reason}；已保存 {} 个采样点到 {}",
                    summary.rows,
                    summary.path.display()
                );
                self.last_recording = Some(summary);
                self.recording_phase = RecordingPhase::Interrupted;
                self.finish_recording_session();
                self.recorder = None;
            }
            Some(RecordingEvent::Failed(error)) => {
                self.recording_status = error;
                self.recording_phase = RecordingPhase::Interrupted;
                self.finish_recording_session();
                self.recorder = None;
            }
            None => {}
        }
    }

    fn request_offline_catalog(&mut self) {
        if self.device_state.is_none() {
            self.offline_status = "请先连接 KM003C，再加载设备离线记录".to_string();
            return;
        }
        if self.offline_busy || self.recorder.is_some() || self.offline_export.is_some() {
            return;
        }
        self.offline_busy = true;
        self.offline_status = "正在加载设备离线记录目录…".to_string();
        if self.cmd_sender.send(UsbCommand::RequestOfflineCatalog).is_err() {
            self.offline_busy = false;
            self.offline_status = "USB 任务不可用".to_string();
        }
    }

    fn download_selected_offline_log(&mut self) {
        if self.device_state.is_none() {
            self.offline_status = "请先连接 KM003C，再下载离线记录".to_string();
            return;
        }
        if self.offline_busy || self.recorder.is_some() || self.offline_export.is_some() {
            return;
        }
        let Some(metadata) = self
            .offline_selected
            .and_then(|index| self.offline_catalog.get(index))
            .cloned()
        else {
            self.offline_status = "请先选择一条离线记录".to_string();
            return;
        };
        self.offline_busy = true;
        self.offline_status = format!(
            "正在从 {} 下载 {} 个采样点…",
            metadata.filename_lossy(),
            metadata.sample_count,
        );
        if self.cmd_sender.send(UsbCommand::DownloadOfflineLog(metadata)).is_err() {
            self.offline_busy = false;
            self.offline_status = "USB 任务不可用".to_string();
        }
    }

    fn export_offline_log(&mut self) {
        if self.offline_export.is_some() {
            return;
        }
        let (Some(view), Some(device)) = (&self.offline_view, &self.offline_device_metadata) else {
            self.offline_status = "请先下载一条离线记录，再导出".to_string();
            return;
        };
        let device_filename = view.log.metadata.filename_lossy();
        let prefix = std::path::Path::new(device_filename.as_ref())
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("offline-log");
        let Some(path) = self.select_recording_path(prefix, "导出 KM003C 离线记录") else {
            return;
        };
        match OfflineExportTask::start(path.clone(), self.recording_format, device.clone(), Arc::clone(view)) {
            Ok(task) => {
                self.offline_status = format!("正在导出到 {}", path.display());
                self.offline_export = Some(task);
            }
            Err(error) => self.offline_status = error,
        }
    }

    fn poll_offline_export(&mut self) {
        let event = self.offline_export.as_mut().and_then(OfflineExportTask::poll_event);
        match event {
            Some(OfflineExportEvent::Finished { path, rows }) => {
                self.offline_status = format!("已导出 {rows} 个采样点到 {}", path.display());
                self.offline_export = None;
            }
            Some(OfflineExportEvent::Failed(error)) => {
                self.offline_status = error;
                self.offline_export = None;
            }
            None => {}
        }
    }

    fn import_recording_dialog(&mut self) {
        if self.recording_import.is_some() {
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .set_title("导入 KM003C 录制")
            .add_filter("KM003C 录制", &["csv", "parquet"])
            .pick_file()
        else {
            return;
        };
        self.start_recording_import(path);
    }

    fn start_recording_import(&mut self, path: PathBuf) {
        if self.recording_import.is_some() {
            return;
        }
        match RecordingImportTask::start(path.clone()) {
            Ok(task) => {
                self.import_status = format!("正在校验并导入 {}", path.display());
                self.recording_import = Some(task);
            }
            Err(error) => self.import_status = error,
        }
    }

    fn poll_recording_import(&mut self) {
        let event = self.recording_import.as_mut().and_then(RecordingImportTask::poll_event);
        match event {
            Some(RecordingImportEvent::Finished(recording)) => {
                let recording = *recording;
                let rows = recording.samples.len();
                let path = recording.path.clone();
                self.imported_recording = Some(recording);
                self.plot_source = PlotSource::Imported;
                self.active_tab = WorkspaceTab::Monitor;
                self.time_window = TimeWindow::All;
                self.chart_viewport.selection = None;
                self.chart_follow_mode = ChartFollowMode::FullSession;
                self.cursor_readout = None;
                self.cursor_pinned = false;
                self.reset_plots_requested = true;
                self.import_status = format!("已导入 {rows} 个采样点：{}", path.display());
                self.recording_import = None;
            }
            Some(RecordingImportEvent::Failed(error)) => {
                self.import_status = format!("导入失败：{error}");
                self.recording_import = None;
            }
            None => {}
        }
    }
}

impl PowerMonitorApp {
    fn live_display_time(&self, absolute_seconds: f64) -> f64 {
        (absolute_seconds - self.live_plot_origin_seconds).max(0.0)
    }

    fn live_absolute_time(&self, display_seconds: f64) -> f64 {
        display_seconds.max(0.0) + self.live_plot_origin_seconds
    }

    fn preferred_follow_mode(&self) -> ChartFollowMode {
        if self.time_window == TimeWindow::All {
            ChartFollowMode::FullSession
        } else {
            ChartFollowMode::LatestWindow
        }
    }

    fn resume_chart_following(&mut self) {
        self.chart_follow_mode = self.preferred_follow_mode();
        self.chart_viewport.selection = None;
    }

    fn enter_manual_chart_view(&mut self) {
        self.chart_follow_mode = ChartFollowMode::Manual;
    }

    fn source_end_time(&self) -> f64 {
        match self.plot_source {
            PlotSource::Live => self
                .data_points
                .back()
                .map_or(0.0, |sample| self.live_display_time(sample.elapsed_seconds())),
            PlotSource::Offline => self
                .offline_view
                .as_ref()
                .and_then(|view| view.samples.last())
                .map_or(0.0, |sample| sample.elapsed_seconds()),
            PlotSource::Imported => self
                .imported_recording
                .as_ref()
                .and_then(|recording| recording.samples.last())
                .map_or(0.0, |sample| sample.elapsed_seconds()),
        }
    }

    fn source_sample_count(&self) -> usize {
        match self.plot_source {
            PlotSource::Live => self.data_points.len(),
            PlotSource::Offline => self.offline_view.as_ref().map_or(0, |view| view.samples.len()),
            PlotSource::Imported => self
                .imported_recording
                .as_ref()
                .map_or(0, |recording| recording.samples.len()),
        }
    }

    fn source_label(&self) -> String {
        let language = self.language;
        match self.plot_source {
            PlotSource::Live => language.pick("实时 AdcQueue", "Live AdcQueue").to_string(),
            PlotSource::Offline => self.offline_view.as_ref().map_or_else(
                || language.pick("设备离线记录", "On-device recording").to_string(),
                |view| {
                    format!(
                        "{} · {}",
                        language.pick("设备记录", "On-device"),
                        view.log.metadata.filename_lossy()
                    )
                },
            ),
            PlotSource::Imported => self.imported_recording.as_ref().map_or_else(
                || language.pick("桌面录制", "Imported recording").to_string(),
                |recording| {
                    recording.path.file_name().and_then(|name| name.to_str()).map_or_else(
                        || language.pick("桌面录制", "Imported recording").to_string(),
                        |name| format!("{} · {name}", language.pick("导入", "Imported")),
                    )
                },
            ),
        }
    }

    fn ensure_chart_selection(&mut self) -> NavigatorSelection {
        let full_end = self.source_end_time().max(0.001);
        if self.reset_plots_requested {
            self.chart_viewport.selection = None;
            self.chart_follow_mode = self.preferred_follow_mode();
        }
        match self.chart_follow_mode {
            ChartFollowMode::FullSession => {
                self.chart_viewport.selection = Some(NavigatorSelection {
                    start_seconds: 0.0,
                    end_seconds: full_end,
                });
            }
            ChartFollowMode::LatestWindow => {
                let width = self.time_window.seconds().unwrap_or(full_end).min(full_end).max(0.001);
                self.chart_viewport.selection = Some(NavigatorSelection {
                    start_seconds: (full_end - width).max(0.0),
                    end_seconds: full_end,
                });
            }
            ChartFollowMode::Manual if self.chart_viewport.selection.is_none() => {
                self.chart_viewport.selection = Some(NavigatorSelection {
                    start_seconds: 0.0,
                    end_seconds: full_end,
                });
            }
            ChartFollowMode::Manual => {}
        }
        let selection = self.chart_viewport.selection.unwrap().clamped(full_end);
        self.chart_viewport.selection = Some(selection);
        selection
    }

    fn displayed_current_readout(&self) -> CursorReadout {
        self.cursor_readout_at(self.source_end_time()).unwrap_or(CursorReadout {
            time_seconds: 0.0,
            voltage: self.current_voltage,
            current: self.current_current.abs(),
            power: self.current_power.abs(),
            approximate: false,
        })
    }

    /// Describes the provenance of the large V/I/P readouts independently
    /// from recording state. KM003C keeps streaming before a recording starts,
    /// so those values must remain visible as live measurements. After a USB
    /// disconnect we retain the last numbers for reference but label them
    /// explicitly instead of presenting stale data as live.
    fn instrument_readout_status(&self) -> &'static str {
        match self.plot_source {
            PlotSource::Imported => self.language.pick("文件", "File"),
            PlotSource::Offline => self.language.pick("离线", "Offline"),
            PlotSource::Live if self.streaming => self.language.pick("实时", "Live"),
            PlotSource::Live
                if self.total_samples > 0
                    || !self.data_points.is_empty()
                    || self.current_voltage != 0.0
                    || self.current_current != 0.0
                    || self.current_power != 0.0 =>
            {
                self.language.pick("最后读数", "Last reading")
            }
            PlotSource::Live => self.language.pick("等待设备", "Waiting"),
        }
    }

    fn source_statistics(&self) -> RecordingSessionStatistics {
        match self.plot_source {
            PlotSource::Live => self.recording_statistics,
            PlotSource::Imported => self
                .imported_recording
                .as_ref()
                .map_or_else(RecordingSessionStatistics::default, |recording| {
                    RecordingSessionStatistics::from_measurements(recording.samples.iter())
                }),
            PlotSource::Offline => {
                let mut statistics = RecordingSessionStatistics::default();
                if let Some(view) = &self.offline_view {
                    for sample in &view.samples {
                        statistics.voltage.push(sample.vbus_uv as f64 / 1_000_000.0);
                        statistics.current.push((sample.ibus_ua as f64 / 1_000_000.0).abs());
                        statistics.power.push((sample.power_uw as f64 / 1_000_000.0).abs());
                    }
                }
                statistics
            }
        }
    }

    fn source_signal_values(&self) -> [Option<f64>; 4] {
        let values = match self.plot_source {
            PlotSource::Live => self
                .data_points
                .back()
                .map(|sample| [sample.dp_uv, sample.dm_uv, sample.cc1_uv, sample.cc2_uv]),
            PlotSource::Imported => self.imported_recording.as_ref().and_then(|recording| {
                recording
                    .samples
                    .last()
                    .map(|sample| [sample.dp_uv, sample.dm_uv, sample.cc1_uv, sample.cc2_uv])
            }),
            PlotSource::Offline => None,
        };
        values.map_or([None; 4], |values| values.map(|value| Some(value as f64 / 1_000_000.0)))
    }

    fn displayed_protocol_state(&self) -> PowerProtocolState {
        if self.plot_source != PlotSource::Live {
            return PowerProtocolState::Unavailable;
        }
        if self.demo_mode {
            return PowerProtocolState::Confirmed(PdContract {
                kind: PdContractKind::Fixed,
                object_position: 2,
                voltage_v: Some(9.0),
                current_a: Some(2.0),
                power_w: None,
            });
        }
        if self.device_state.is_none() {
            return PowerProtocolState::Disconnected;
        }
        self.pd_decoder.display_state(self.pd_connection.connected())
    }

    fn source_vip_points(
        &self,
        selection: NavigatorSelection,
        max_points: usize,
        filter: DisplayFilter,
    ) -> [Vec<[f64; 2]>; 3] {
        let in_range = |time: f64| time >= selection.start_seconds && time <= selection.end_seconds;
        let (mut voltage, mut current, mut power) = (Vec::new(), Vec::new(), Vec::new());
        match self.plot_source {
            PlotSource::Live => {
                let detailed_start = self
                    .data_points
                    .front()
                    .map_or(f64::INFINITY, |sample| sample.elapsed_seconds());
                for point in &self.navigator_history.points {
                    if point.time_seconds + f64::EPSILON < self.live_plot_origin_seconds
                        || point.time_seconds >= detailed_start
                    {
                        continue;
                    }
                    let time = self.live_display_time(point.time_seconds);
                    if in_range(time) {
                        for value in [point.minimums[0], point.values[0], point.maximums[0]] {
                            voltage.push([time, value]);
                        }
                        for value in [point.minimums[1], point.values[1], point.maximums[1]] {
                            current.push([time, value]);
                        }
                        for value in [point.minimums[2], point.values[2], point.maximums[2]] {
                            power.push([time, value]);
                        }
                    }
                }
                for sample in &self.data_points {
                    if sample.elapsed_seconds() + f64::EPSILON < self.live_plot_origin_seconds {
                        continue;
                    }
                    let time = self.live_display_time(sample.elapsed_seconds());
                    if !in_range(time) {
                        continue;
                    }
                    voltage.push([time, sample.vbus_uv as f64 / 1_000_000.0]);
                    current.push([time, (sample.ibus_ua as f64 / 1_000_000.0).abs()]);
                    power.push([time, (sample.power_uw as f64 / 1_000_000.0).abs()]);
                }
            }
            PlotSource::Offline => {
                if let Some(view) = &self.offline_view {
                    for sample in view.samples.iter().filter(|sample| in_range(sample.elapsed_seconds())) {
                        let time = sample.elapsed_seconds();
                        voltage.push([time, sample.vbus_uv as f64 / 1_000_000.0]);
                        current.push([time, (sample.ibus_ua as f64 / 1_000_000.0).abs()]);
                        power.push([time, (sample.power_uw as f64 / 1_000_000.0).abs()]);
                    }
                }
            }
            PlotSource::Imported => {
                if let Some(recording) = &self.imported_recording {
                    for sample in recording
                        .samples
                        .iter()
                        .filter(|sample| in_range(sample.elapsed_seconds()))
                    {
                        let time = sample.elapsed_seconds();
                        voltage.push([time, sample.vbus_uv as f64 / 1_000_000.0]);
                        current.push([time, (sample.ibus_ua as f64 / 1_000_000.0).abs()]);
                        power.push([time, (sample.power_uw as f64 / 1_000_000.0).abs()]);
                    }
                }
            }
        }
        [
            min_max_downsample(apply_display_filter(voltage, filter), max_points),
            min_max_downsample(apply_display_filter(current, filter), max_points),
            min_max_downsample(apply_display_filter(power, filter), max_points),
        ]
    }

    fn source_metric_points(
        &self,
        metric: PlotMetric,
        selection: NavigatorSelection,
        max_points: usize,
    ) -> Vec<[f64; 2]> {
        let in_range = |time: f64| time >= selection.start_seconds && time <= selection.end_seconds;
        let points = match self.plot_source {
            PlotSource::Live => self
                .data_points
                .iter()
                .filter(|sample| sample.elapsed_seconds() + f64::EPSILON >= self.live_plot_origin_seconds)
                .filter_map(|sample| {
                    let time = self.live_display_time(sample.elapsed_seconds());
                    in_range(time).then(|| [time, metric.value(sample)])
                })
                .collect(),
            PlotSource::Offline => self
                .offline_view
                .iter()
                .flat_map(|view| &view.samples)
                .filter(|sample| in_range(sample.elapsed_seconds()))
                .filter_map(|sample| {
                    sample
                        .metric_value(metric)
                        .map(|value| [sample.elapsed_seconds(), value])
                })
                .collect(),
            PlotSource::Imported => self
                .imported_recording
                .iter()
                .flat_map(|recording| recording.samples.iter())
                .filter(|sample| in_range(sample.elapsed_seconds()))
                .map(|sample| [sample.elapsed_seconds(), metric.value(sample)])
                .collect(),
        };
        min_max_downsample(points, max_points)
    }

    fn navigator_vip_points(&self, max_points: usize) -> [Vec<[f64; 2]>; 3] {
        let mut points = [Vec::new(), Vec::new(), Vec::new()];
        match self.plot_source {
            PlotSource::Live => {
                for point in &self.navigator_history.points {
                    if point.time_seconds + f64::EPSILON < self.live_plot_origin_seconds {
                        continue;
                    }
                    let time = self.live_display_time(point.time_seconds);
                    for (index, series) in points.iter_mut().enumerate() {
                        for value in [point.minimums[index], point.values[index], point.maximums[index]] {
                            series.push([time, value]);
                        }
                    }
                }
            }
            PlotSource::Offline => {
                for sample in self.offline_view.iter().flat_map(|view| &view.samples) {
                    let time = sample.elapsed_seconds();
                    points[0].push([time, sample.vbus_uv as f64 / 1_000_000.0]);
                    points[1].push([time, (sample.ibus_ua as f64 / 1_000_000.0).abs()]);
                    points[2].push([time, (sample.power_uw as f64 / 1_000_000.0).abs()]);
                }
            }
            PlotSource::Imported => {
                for sample in self
                    .imported_recording
                    .iter()
                    .flat_map(|recording| recording.samples.iter())
                {
                    let time = sample.elapsed_seconds();
                    points[0].push([time, sample.vbus_uv as f64 / 1_000_000.0]);
                    points[1].push([time, (sample.ibus_ua as f64 / 1_000_000.0).abs()]);
                    points[2].push([time, (sample.power_uw as f64 / 1_000_000.0).abs()]);
                }
            }
        }
        points.map(|series| min_max_downsample(series, max_points))
    }
}

impl PowerMonitorApp {
    fn show_workbench(&mut self, ui: &mut egui::Ui) {
        let usb_backlog = self.process_messages();
        self.update_demo_data();
        if usb_backlog {
            ui.ctx().request_repaint();
        } else if self.streaming && self.plot_source == PlotSource::Live {
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        } else {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }

        self.show_workspace_header(ui);
        if self.active_tab == WorkspaceTab::Monitor {
            self.show_monitor_toolbar(ui);
        }
        self.show_status_bar(ui);

        match self.active_tab {
            WorkspaceTab::Monitor => self.show_monitor_page(ui),
            WorkspaceTab::PdAnalysis => self.show_pd_analysis_page(ui),
        }
        self.show_settings_window(ui.ctx());
        self.show_advanced_analysis_window(ui.ctx());
        self.show_disconnect_confirmation(ui.ctx());
        self.show_clear_data_confirmation(ui.ctx());
    }

    fn show_disconnect_confirmation(&mut self, ctx: &egui::Context) {
        if !self.disconnect_confirmation {
            return;
        }
        let response = egui::Modal::new(egui::Id::new("disconnect_confirmation"))
            .frame(
                egui::Frame::popup(ctx.style_of(egui::Theme::Dark).as_ref())
                    .fill(theme::PANEL_RAISED)
                    .stroke(egui::Stroke::new(1.0, theme::RECORDING.gamma_multiply(0.8)))
                    .corner_radius(egui::CornerRadius::same(8))
                    .inner_margin(egui::Margin::same(18)),
            )
            .show(ctx, |ui| {
                let language = self.language;
                ui.set_min_width(310.0);
                ui.label(
                    egui::RichText::new(language.pick("断开 KM003C？", "Disconnect KM003C?"))
                        .strong()
                        .size(18.0),
                );
                ui.add_space(6.0);
                ui.label(language.pick(
                    "实时采样将停止；正在录制的文件会先安全结束。",
                    "Live sampling will stop. Any active recording will be finalized safely first.",
                ));
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    if ui.button(language.pick("取消", "Cancel")).clicked() {
                        self.disconnect_confirmation = false;
                    }
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new(language.pick("确认断开", "Disconnect"))
                                    .color(egui::Color32::WHITE),
                            )
                            .fill(theme::RECORDING.gamma_multiply(0.8))
                            .stroke(egui::Stroke::new(1.0, theme::RECORDING)),
                        )
                        .clicked()
                    {
                        self.disconnect_confirmation = false;
                        self.disconnect_requested = true;
                        let _ = self.cmd_sender.send(UsbCommand::Disconnect);
                    }
                });
            });
        if response.backdrop_response.clicked() {
            self.disconnect_confirmation = false;
        }
    }

    fn show_clear_data_confirmation(&mut self, ctx: &egui::Context) {
        if !self.clear_data_confirmation {
            return;
        }
        let response = egui::Modal::new(egui::Id::new("clear_data_confirmation"))
            .frame(
                egui::Frame::popup(ctx.style_of(egui::Theme::Dark).as_ref())
                    .fill(theme::PANEL_RAISED)
                    .stroke(egui::Stroke::new(1.0, theme::POWER.gamma_multiply(0.75)))
                    .corner_radius(egui::CornerRadius::same(8))
                    .inner_margin(egui::Margin::same(18)),
            )
            .show(ctx, |ui| {
                let language = self.language;
                ui.set_min_width(340.0);
                ui.label(
                    egui::RichText::new(language.pick("清空实时数据？", "Clear live data?"))
                        .strong()
                        .size(18.0),
                );
                ui.add_space(6.0);
                ui.label(language.pick(
                    "当前实时缓冲区和图表会被清空；已保存录制、导入文件和设备离线记录不会被删除。",
                    "The live buffer and chart will be cleared. Saved recordings, imported files, and on-device recordings will not be deleted.",
                ));
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    if ui.button(language.pick("取消", "Cancel")).clicked() {
                        self.clear_data_confirmation = false;
                    }
                    if ui
                        .add(
                            egui::Button::new(language.pick("确认清空", "Clear live data"))
                                .fill(theme::POWER.gamma_multiply(0.18))
                                .stroke(egui::Stroke::new(1.0, theme::POWER)),
                        )
                        .clicked()
                    {
                        self.clear_data_confirmation = false;
                        self.clear_data();
                    }
                });
            });
        if response.backdrop_response.clicked() {
            self.clear_data_confirmation = false;
        }
    }

    fn show_workspace_header(&mut self, ui: &mut egui::Ui) {
        let language = self.language;
        let compact = ui.ctx().content_rect().width() < 1240.0;
        egui::Panel::top("workspace_header")
            .exact_size(48.0)
            .frame(
                egui::Frame::NONE
                    .fill(theme::BACKPLANE)
                    .stroke(egui::Stroke::new(1.0, theme::DIVIDER))
                    .inner_margin(egui::Margin::symmetric(12, 7)),
            )
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.spacing_mut().interact_size.y = 32.0;
                ui.horizontal(|ui| {
                    for (tab, label) in [
                        (WorkspaceTab::Monitor, language.pick("监控", "Monitor")),
                        (WorkspaceTab::PdAnalysis, language.pick("PD 分析", "PD Analysis")),
                    ] {
                        let active = self.active_tab == tab;
                        let button =
                            egui::Button::new(egui::RichText::new(label).strong().size(15.0).color(if active {
                                egui::Color32::WHITE
                            } else {
                                theme::MUTED_TEXT
                            }))
                            .fill(if active {
                                theme::PANEL_RAISED
                            } else {
                                egui::Color32::TRANSPARENT
                            })
                            .stroke(if active {
                                egui::Stroke::new(1.0, theme::DIVIDER)
                            } else {
                                egui::Stroke::new(1.0, egui::Color32::TRANSPARENT)
                            })
                            .corner_radius(egui::CornerRadius::same(5))
                            .min_size(egui::vec2(if compact { 88.0 } else { 104.0 }, 32.0));
                        if ui.add(button).clicked() {
                            self.active_tab = tab;
                        }
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(
                                egui::Button::new(language.pick("设置", "Settings"))
                                    .min_size(egui::vec2(if compact { 52.0 } else { 68.0 }, 32.0)),
                            )
                            .on_hover_text(language.pick(
                                "设置、设备信息与高级分析",
                                "Settings, device information, and advanced analysis",
                            ))
                            .clicked()
                        {
                            self.settings_open = !self.settings_open;
                        }

                        let connection_label = if self.streaming {
                            language.pick("断开", "Disconnect")
                        } else {
                            language.pick("连接", "Connect")
                        };
                        let mut connection_button =
                            egui::Button::new(connection_label).min_size(egui::vec2(72.0, 32.0));
                        if self.streaming {
                            connection_button = connection_button
                                .fill(theme::RECORDING.gamma_multiply(0.08))
                                .stroke(egui::Stroke::new(1.0, theme::RECORDING.gamma_multiply(0.9)));
                        }
                        if ui.add(connection_button).clicked() {
                            if self.streaming {
                                self.disconnect_confirmation = true;
                            } else {
                                self.request_connect();
                            }
                        }
                        if ui
                            .add_enabled(
                                !self.streaming && self.phase != ConnectionPhase::Connecting,
                                egui::Button::new(language.pick("刷新", "Refresh"))
                                    .min_size(egui::vec2(if compact { 52.0 } else { 64.0 }, 32.0)),
                            )
                            .on_hover_text(language.pick("重新搜索设备", "Search for the device again"))
                            .clicked()
                        {
                            self.request_connect();
                        }

                        let phase_color = match self.phase {
                            ConnectionPhase::Streaming | ConnectionPhase::Searching | ConnectionPhase::Connecting => {
                                theme::TEXT_SECONDARY
                            }
                            ConnectionPhase::NoDevice | ConnectionPhase::Disconnected => theme::MUTED_TEXT,
                            ConnectionPhase::DeviceBusy | ConnectionPhase::ConnectionError => theme::RECORDING,
                        };
                        egui::Frame::NONE
                            .fill(theme::PANEL_RAISED)
                            .stroke(egui::Stroke::new(1.0, theme::DIVIDER))
                            .corner_radius(egui::CornerRadius::same(5))
                            .inner_margin(egui::Margin::symmetric(if compact { 9 } else { 12 }, 6))
                            .show(ui, |ui| {
                                let model = self
                                    .device_state
                                    .as_ref()
                                    .map_or("KM003C", |state| state.info.model.as_str());
                                ui.set_max_width(if compact { 178.0 } else { 248.0 });
                                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                    ui.colored_label(phase_color, "●");
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(format!(
                                                "{model} · {}",
                                                i18n::connection_status(self.language, self.phase)
                                            ))
                                            .strong()
                                            .color(phase_color),
                                        )
                                        .truncate(),
                                    );
                                });
                            });

                        egui::ComboBox::from_id_salt("workspace_language")
                            .width(72.0)
                            .selected_text(self.language.short_name())
                            .show_ui(ui, |ui| {
                                ui.set_min_width(132.0);
                                for option in Language::ALL {
                                    ui.selectable_value(&mut self.language, option, option.native_name());
                                }
                            })
                            .response
                            .on_hover_text(language.pick("切换界面语言", "Change interface language"));
                    });
                });
            });
    }
}

impl PowerMonitorApp {
    fn show_monitor_toolbar(&mut self, ui: &mut egui::Ui) {
        let density = toolbar_density(ui.ctx().content_rect().width());
        let compact = density != ToolbarDensity::Full;
        let narrow = density == ToolbarDensity::Narrow;
        let language = self.language;
        egui::Panel::top("monitor_toolbar")
            .exact_size(52.0)
            .frame(
                egui::Frame::NONE
                    .fill(theme::PANEL)
                    .stroke(egui::Stroke::new(1.0, theme::DIVIDER))
                    .inner_margin(egui::Margin::symmetric(10, 8)),
            )
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.spacing_mut().interact_size.y = 32.0;
                ui.horizontal(|ui| {
                    let record_label = match self.recording_phase {
                        RecordingPhase::Recording => language.pick("Ⅱ 暂停记录", "Ⅱ Pause"),
                        RecordingPhase::Paused => language.pick("▶ 继续记录", "▶ Resume"),
                        RecordingPhase::Interrupted if self.recording_session => {
                            language.pick("▶ 恢复记录", "▶ Recover")
                        }
                        RecordingPhase::WaitingForReconnect => language.pick("等待重连…", "Waiting to reconnect…"),
                        RecordingPhase::Recovering => language.pick("正在恢复…", "Recovering…"),
                        RecordingPhase::Finalizing => language.pick("正在保存…", "Saving…"),
                        _ => language.pick("● 开始记录", "● Record"),
                    };
                    let can_toggle = self.recording_phase != RecordingPhase::Finalizing
                        && !matches!(
                            self.recording_phase,
                            RecordingPhase::WaitingForReconnect | RecordingPhase::Recovering
                        )
                        && (self.recording_session || (self.streaming && self.recorder.is_none()));
                    let record_text_color = match self.recording_phase {
                        RecordingPhase::Recording => theme::RECORDING,
                        RecordingPhase::Paused => theme::POWER,
                        RecordingPhase::Interrupted => theme::RECORDING,
                        RecordingPhase::WaitingForReconnect | RecordingPhase::Recovering => theme::POWER,
                        RecordingPhase::Finalizing => theme::TEXT_SECONDARY,
                        _ => theme::RECORDING,
                    };
                    if ui
                        .add_enabled(
                            can_toggle,
                            egui::Button::new(egui::RichText::new(record_label).strong().color(record_text_color))
                                .fill(theme::PANEL_RAISED)
                                .stroke(egui::Stroke::new(
                                    1.0,
                                    if self.recording_session {
                                        record_text_color.gamma_multiply(0.8)
                                    } else {
                                        theme::DIVIDER
                                    },
                                ))
                                .min_size(egui::vec2(
                                    match density {
                                        ToolbarDensity::Full => 118.0,
                                        ToolbarDensity::Compact => 104.0,
                                        ToolbarDensity::Narrow => 96.0,
                                    },
                                    32.0,
                                )),
                        )
                        .clicked()
                    {
                        match self.recording_phase {
                            RecordingPhase::Recording => self.pause_recording(),
                            RecordingPhase::Paused | RecordingPhase::Interrupted => {
                                self.resume_recording();
                            }
                            RecordingPhase::Idle | RecordingPhase::Saved => self.start_recording(),
                            RecordingPhase::WaitingForReconnect
                            | RecordingPhase::Recovering
                            | RecordingPhase::Finalizing => {}
                        }
                    }

                    if ui
                        .add_enabled(
                            self.recording_session && self.recording_phase != RecordingPhase::Finalizing,
                            egui::Button::new(if compact {
                                language.pick("保存", "Save")
                            } else {
                                language.pick("保存录制", "Save recording")
                            })
                                .min_size(egui::vec2(if compact { 60.0 } else { 88.0 }, 32.0)),
                        )
                        .clicked()
                    {
                        self.save_recording();
                    }

                    if self.recording_session {
                        let color = if self.recording_phase == RecordingPhase::Recording {
                            theme::RECORDING
                        } else if self.recording_phase == RecordingPhase::Paused {
                            theme::POWER
                        } else {
                            theme::TEXT_SECONDARY
                        };
                        ui.colored_label(
                            color,
                            egui::RichText::new(format!(
                                "{} {}",
                                if self.recording_phase == RecordingPhase::Recording {
                                    "●"
                                } else {
                                    "Ⅱ"
                                },
                                format_recording_duration(self.displayed_recording_duration())
                            ))
                            .monospace()
                            .size(20.0)
                            .strong(),
                        );
                    }

                    ui.add(egui::Separator::default().vertical().spacing(8.0));
                    let previous_rate = self.selected_rate;
                    egui::ComboBox::from_id_salt("toolbar_sample_rate")
                        .width(if compact { 72.0 } else { 88.0 })
                        .selected_text(self.selected_rate.label())
                        .show_ui(ui, |ui| {
                            for rate in SampleRateOption::all() {
                                ui.selectable_value(&mut self.selected_rate, *rate, rate.label());
                            }
                        });
                    if self.selected_rate != previous_rate && self.device_state.is_some() && self.recorder.is_none() {
                        let _ = self
                            .cmd_sender
                            .send(UsbCommand::SetSampleRate(self.selected_rate.to_graph_rate()));
                    }
                    if self.streaming && self.current_rate != self.selected_rate {
                        ui.label(
                            egui::RichText::new(format!(
                                "{} {}",
                                language.pick("实际", "Actual"),
                                self.current_rate.label()
                            ))
                                .small()
                                .monospace()
                                .color(theme::TEXT_SECONDARY),
                        )
                        .on_hover_text(language.pick(
                            "设备实际采样率与所选采样率不一致",
                            "The device sample rate differs from the selected rate",
                        ));
                    }

                    let auto_label = match density {
                        ToolbarDensity::Full => language.pick("自动暂停/继续", "Auto pause/resume"),
                        ToolbarDensity::Compact => language.pick("自动控制", "Auto control"),
                        ToolbarDensity::Narrow => language.pick("自动", "Auto"),
                    };
                    ui.checkbox(&mut self.auto_pause_enabled, auto_label)
                        .on_hover_text(language.pick(
                            "录制时按设置的功率、电流或电压阈值自动暂停；仅自动暂停可自动继续",
                            "Pause using the configured power, current, or voltage threshold. Only an automatic pause can resume automatically.",
                        ));

                    ui.add(egui::Separator::default().vertical().spacing(8.0));
                    if ui
                        .add(
                            egui::Button::new(if compact {
                                language.pick("导入", "Import")
                            } else {
                                language.pick("↓ 导入", "↓ Import")
                            })
                            .min_size(egui::vec2(68.0, 32.0)),
                        )
                        .clicked()
                    {
                        self.import_recording_dialog();
                    }

                    let can_export = self.plot_source == PlotSource::Live
                        && !self.data_points.is_empty()
                        && self.recorder.is_none();
                    if !narrow
                        && ui
                            .add_enabled(
                                can_export,
                                egui::Button::new(language.pick("导出", "Export"))
                                    .min_size(egui::vec2(68.0, 32.0)),
                            )
                            .on_hover_text(language.pick(
                                "按设置中的格式导出当前实时缓冲区（CSV 或 Parquet）",
                                "Export the current live buffer in the selected format (CSV or Parquet)",
                            ))
                            .clicked()
                    {
                        self.export_buffer();
                    }

                    if density == ToolbarDensity::Full
                        && ui
                            .add_enabled(
                                self.recorder.is_none(),
                                egui::Button::new(language.pick("清空数据", "Clear data"))
                                    .min_size(egui::vec2(82.0, 32.0)),
                            )
                            .on_hover_text(language.pick(
                                "清空当前实时缓冲区；已保存录制不会被删除",
                                "Clear the current live buffer. Saved recordings are not deleted.",
                            ))
                            .clicked()
                    {
                        self.clear_data_confirmation = true;
                    }

                    if !narrow
                        && ui
                            .add(
                                egui::Button::new(language.pick("恢复视图", "Reset view"))
                                    .min_size(egui::vec2(if compact { 76.0 } else { 88.0 }, 32.0)),
                            )
                            .clicked()
                    {
                        self.cursor_readout = None;
                        self.cursor_pinned = false;
                        self.resume_chart_following();
                        self.reset_plots_requested = true;
                    }

                    if density != ToolbarDensity::Full {
                        ui.menu_button(language.pick("更多", "More"), |ui| {
                            if narrow
                                && ui
                                    .add_enabled(can_export, egui::Button::new(language.pick("导出缓冲区", "Export buffer")))
                                    .clicked()
                            {
                                self.export_buffer();
                                ui.close();
                            }
                            if ui
                                .add_enabled(
                                    self.recorder.is_none(),
                                    egui::Button::new(language.pick("清空实时数据…", "Clear live data…")),
                                )
                                .clicked()
                            {
                                self.clear_data_confirmation = true;
                                ui.close();
                            }
                            if narrow && ui.button(language.pick("恢复视图", "Reset view")).clicked() {
                                self.cursor_readout = None;
                                self.cursor_pinned = false;
                                self.resume_chart_following();
                                self.reset_plots_requested = true;
                                ui.close();
                            }
                            ui.separator();
                            if ui.button(language.pick("自动暂停设置…", "Auto-pause settings…")).clicked() {
                                self.settings_page = SettingsPage::Recording;
                                self.settings_open = true;
                                ui.close();
                            }
                        });
                    }
                });
            });
    }

    fn show_status_bar(&mut self, ui: &mut egui::Ui) {
        let language = self.language;
        egui::Panel::bottom("workbench_status_bar")
            .exact_size(28.0)
            .frame(
                egui::Frame::NONE
                    .fill(theme::BACKPLANE)
                    .stroke(egui::Stroke::new(1.0, theme::DIVIDER))
                    .inner_margin(egui::Margin::symmetric(10, 4)),
            )
            .show(ui, |ui| {
                let phase_color = match self.recording_phase {
                    RecordingPhase::Recording => theme::RECORDING,
                    RecordingPhase::Paused => theme::TEXT_SECONDARY,
                    RecordingPhase::WaitingForReconnect | RecordingPhase::Recovering => theme::POWER,
                    RecordingPhase::Interrupted => theme::RECORDING,
                    RecordingPhase::Saved => theme::TEXT_SECONDARY,
                    _ => theme::MUTED_TEXT,
                };
                ui.columns(3, |columns| {
                    columns[0].horizontal(|ui| {
                        ui.colored_label(
                            phase_color,
                            format!("● {}", self.recording_phase.localized_label(language)),
                        );
                        ui.monospace(format_recording_duration(self.displayed_source_duration()));
                    });
                    columns[1].with_layout(
                        egui::Layout::left_to_right(egui::Align::Center).with_main_justify(true),
                        |ui| {
                            ui.monospace(format!(
                                "{} · {} pts",
                                self.current_rate.label(),
                                match self.plot_source {
                                    PlotSource::Live => self.total_samples,
                                    PlotSource::Offline | PlotSource::Imported => self.source_sample_count() as u64,
                                }
                            ));
                            if self.demo_mode {
                                ui.colored_label(theme::TEXT_MUTED, "DEMO");
                            }
                        },
                    );
                    columns[2].with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let data_complete = self.dropped_samples == 0 && self.discarded_sequence_samples == 0;
                        let (color, label) = if data_complete {
                            (theme::CURRENT, language.pick("数据完整", "Data complete").to_string())
                        } else {
                            (
                                theme::POWER,
                                format!(
                                    "{} {} · {} {}",
                                    language.pick("缺失", "Missing"),
                                    self.dropped_samples,
                                    language.pick("丢弃", "Discarded"),
                                    self.discarded_sequence_samples,
                                ),
                            )
                        };
                        ui.colored_label(color, format!("● {label}"));
                    });
                });
            });
    }

    fn displayed_source_duration(&self) -> Duration {
        match self.plot_source {
            PlotSource::Live => self.displayed_recording_duration(),
            PlotSource::Offline | PlotSource::Imported => Duration::from_secs_f64(self.source_end_time().max(0.0)),
        }
    }
}

impl PowerMonitorApp {
    fn show_monitor_page(&mut self, ui: &mut egui::Ui) {
        let language = self.language;
        // macOS title bars and the two workbench toolbars leave less usable
        // vertical space than the outer viewport suggests. Switch before the
        // left rail starts clipping its CC rows on common 1160×768 windows,
        // while the planned 1280×820 default still keeps the full layout.
        let compact = uses_compact_monitor_layout(ui.available_width(), ui.available_height());
        let rail_width = match (compact, self.language) {
            (true, Language::English) => 252.0,
            (true, Language::SimplifiedChinese) => 238.0,
            (false, Language::English) => 292.0,
            (false, Language::SimplifiedChinese) => 282.0,
        };
        egui::Panel::left("instrument_rail")
            .resizable(false)
            .exact_size(rail_width)
            .frame(
                egui::Frame::NONE
                    .fill(theme::BACKPLANE)
                    .stroke(egui::Stroke::new(1.0, theme::DIVIDER))
                    .inner_margin(egui::Margin::symmetric(if compact { 8 } else { 12 }, 10)),
            )
            .show(ui, |ui| self.show_instrument_rail(ui, compact));

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(theme::BACKPLANE)
                    .inner_margin(egui::Margin::symmetric(10, 10)),
            )
            .show(ui, |ui| {
                let watermark_rect = ui.max_rect();
                if self.monitor_chart_visible() {
                    self.show_combined_monitor_chart(ui, compact);
                } else {
                    self.show_recording_workspace_idle(ui, compact);
                }
                if self.demo_mode {
                    ui.painter().text(
                        watermark_rect.right_bottom() - egui::vec2(16.0, 12.0),
                        egui::Align2::RIGHT_BOTTOM,
                        language.pick("DEMO · 演示数据", "DEMO DATA"),
                        egui::FontId::monospace(if compact { 13.0 } else { 15.0 }),
                        theme::TEXT_MUTED.gamma_multiply(0.48),
                    );
                }
            });
    }

    fn show_recording_workspace_idle(&mut self, ui: &mut egui::Ui, compact: bool) {
        let language = self.language;
        let available = ui.available_size();
        egui::Frame::NONE
            .fill(theme::PANEL)
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::same(20))
            .show(ui, |ui| {
                ui.set_min_size(available - egui::vec2(40.0, 40.0));
                ui.with_layout(
                    egui::Layout::top_down(egui::Align::Center).with_main_align(egui::Align::Center),
                    |ui| {
                        ui.label(
                            egui::RichText::new(if self.last_recording.is_some() {
                                language.pick("上一段录制已结束", "The previous recording has ended")
                            } else if self.streaming {
                                language.pick("实时读数已就绪", "Live readings are ready")
                            } else {
                                language.pick("等待 KM003C 采样", "Waiting for KM003C samples")
                            })
                            .size(if compact { 21.0 } else { 26.0 })
                            .strong(),
                        );
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new(if self.last_recording.is_some() {
                                language.pick(
                                    "左侧保留上一段统计；开始新录制后显示新的完整波形。",
                                    "The previous statistics remain on the left. Start a new recording to begin a new full trace.",
                                )
                            } else {
                                language.pick(
                                    "电压、电流和功率继续实时显示；开始记录后才展开波形、窗口统计和时间导航。",
                                    "Voltage, current, and power remain live. The trace, window statistics, and navigator appear after recording starts.",
                                )
                            })
                            .color(theme::MUTED_TEXT),
                        );
                        ui.add_space(16.0);
                        egui::Frame::NONE
                            .fill(theme::PANEL_RAISED)
                            .stroke(egui::Stroke::new(1.0, theme::DIVIDER))
                            .corner_radius(egui::CornerRadius::same(6))
                            .inner_margin(egui::Margin::symmetric(14, 8))
                            .show(ui, |ui| {
                                ui.label(
                                    egui::RichText::new(language.pick(
                                        "使用顶部“开始记录”进入波形工作区",
                                        "Select Record in the toolbar to open the trace workspace",
                                    ))
                                        .color(theme::TEXT_PRIMARY)
                                        .strong(),
                                );
                            });
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new(language.pick(
                                "导入 CSV / Parquet 或打开设备离线记录时，也会进入波形回放。",
                                "Importing CSV/Parquet or opening an on-device recording also opens trace playback.",
                            ))
                                .small()
                                .color(theme::MUTED_TEXT.gamma_multiply(0.8)),
                        );
                    },
                );
            });
    }

    fn show_instrument_rail(&mut self, ui: &mut egui::Ui, compact: bool) {
        let language = self.language;
        let current = self.displayed_current_readout();
        let readout_status = self.instrument_readout_status();
        let statistics = self.source_statistics();
        instrument_card(
            ui,
            InstrumentCardData {
                label: language.pick("电压", "Voltage"),
                value: current.voltage,
                unit: MeasurementUnit::Voltage,
                color: theme::VOLTAGE,
                statistics: statistics.voltage.readout(),
                compact,
                language,
                readout_status,
            },
        );
        ui.add_space(if compact { 8.0 } else { 12.0 });
        instrument_card(
            ui,
            InstrumentCardData {
                label: language.pick("电流", "Current"),
                value: current.current,
                unit: MeasurementUnit::Current,
                color: theme::CURRENT,
                statistics: statistics.current.readout(),
                compact,
                language,
                readout_status,
            },
        );
        ui.add_space(if compact { 8.0 } else { 12.0 });
        instrument_card(
            ui,
            InstrumentCardData {
                label: language.pick("功率", "Power"),
                value: current.power,
                unit: MeasurementUnit::Power,
                color: theme::POWER,
                statistics: statistics.power.readout(),
                compact,
                language,
                readout_status,
            },
        );

        ui.add_space(if compact { 8.0 } else { 12.0 });
        let accumulated = if self.plot_source == PlotSource::Live {
            AccumulatedReadout {
                cumulative_energy_uwh: self.displayed_cumulative_energy_uwh(),
                capacity_uah: self.displayed_recording_capacity_uah(),
                net_energy_uwh: self.displayed_recording_net_energy_uwh(),
            }
        } else {
            self.accumulated_readout().unwrap_or(AccumulatedReadout {
                cumulative_energy_uwh: 0.0,
                capacity_uah: 0.0,
                net_energy_uwh: 0.0,
            })
        };
        let accumulated_available = match self.plot_source {
            PlotSource::Live => self.recording_session || self.last_recording_duration.is_some(),
            PlotSource::Offline | PlotSource::Imported => self.source_sample_count() > 0,
        };
        let energy_presentation =
            EnergyPresentation::for_values([accumulated.cumulative_energy_uwh, accumulated.net_energy_uwh]);
        let duration_text = if accumulated_available {
            format_recording_duration(self.displayed_source_duration())
        } else {
            "—".to_string()
        };
        let cumulative_energy_text = if accumulated_available {
            energy_presentation.format(accumulated.cumulative_energy_uwh)
        } else {
            "—".to_string()
        };
        let capacity_text = if accumulated_available {
            format_capacity(accumulated.capacity_uah)
        } else {
            "—".to_string()
        };
        let net_energy_text = if accumulated_available {
            energy_presentation.format_directional(accumulated.net_energy_uwh)
        } else {
            "—".to_string()
        };
        let accumulated_width = ui.available_width();
        egui::Frame::NONE
            .fill(theme::PANEL_RAISED)
            .stroke(egui::Stroke::new(1.0, theme::DIVIDER))
            .corner_radius(egui::CornerRadius::same(6))
            .inner_margin(egui::Margin::symmetric(10, if compact { 6 } else { 8 }))
            .show(ui, |ui| {
                ui.set_min_width((accumulated_width - 20.0).max(120.0));
                ui.horizontal(|ui| {
                    let duration_width = 94.0;
                    let title_width = (ui.available_width() - duration_width - 4.0).max(76.0);
                    ui.allocate_ui_with_layout(
                        egui::vec2(title_width, 20.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(language.pick("录制累计", "Session totals"))
                                        .strong()
                                        .size(16.0),
                                )
                                .truncate(),
                            );
                        },
                    );
                    ui.allocate_ui_with_layout(
                        egui::vec2(duration_width, 20.0),
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            ui.monospace(&duration_text);
                        },
                    );
                });
                egui::Grid::new("recording_accumulated_grid")
                    .num_columns(2)
                    .spacing([8.0, if compact { 2.0 } else { 4.0 }])
                    .show(ui, |ui| {
                        ui.colored_label(theme::POWER, language.pick("累计能量", "Energy"));
                        ui.monospace(&cumulative_energy_text);
                        ui.end_row();
                        ui.colored_label(theme::CURRENT, language.pick("累计容量", "Capacity"));
                        ui.monospace(&capacity_text);
                        ui.end_row();
                        ui.colored_label(theme::POWER, language.pick("净能量", "Net energy"));
                        ui.monospace(&net_energy_text);
                        ui.end_row();
                    });
            });

        ui.add_space(if compact { 8.0 } else { 12.0 });
        let [dp, dm, cc1, cc2] = self.source_signal_values();
        let protocol_state = self.displayed_protocol_state();
        let protocol_summary = match protocol_state {
            PowerProtocolState::Confirmed(contract) | PowerProtocolState::Negotiating(contract) => {
                contract.localized_summary(language)
            }
            PowerProtocolState::PdDetected => language
                .pick("检测到 USB PD · 等待合同", "USB PD detected · Waiting for contract")
                .to_string(),
            PowerProtocolState::TraditionalUnconfirmed => language
                .pick(
                    "协议未确认 · 不推测 QC / VOOC / UFCS",
                    "Protocol unconfirmed · QC / VOOC / UFCS not inferred",
                )
                .to_string(),
            PowerProtocolState::Waiting => language
                .pick("等待 Type-C / PD 协商", "Waiting for Type-C / PD negotiation")
                .to_string(),
            PowerProtocolState::Disconnected => language
                .pick("未检测到受测设备", "No device detected on the test port")
                .to_string(),
            PowerProtocolState::Unavailable => language
                .pick(
                    "离线文件未保存 PD 合同",
                    "The offline file does not contain a PD contract",
                )
                .to_string(),
        };
        let protocol_width = ui.available_width();
        let protocol_card = egui::Frame::NONE
            .fill(theme::PANEL_RAISED)
            .stroke(egui::Stroke::new(1.0, theme::DIVIDER))
            .corner_radius(egui::CornerRadius::same(6))
            .inner_margin(egui::Margin::symmetric(9, if compact { 5 } else { 7 }))
            .show(ui, |ui| {
                ui.set_min_width((protocol_width - 18.0).max(120.0));
                if compact {
                    ui.label(
                        egui::RichText::new(language.pick("当前协议", "Active protocol"))
                            .strong()
                            .size(16.0),
                    );
                } else {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(language.pick("当前协议", "Active protocol"))
                                .strong()
                                .size(16.0),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                egui::RichText::new(protocol_state.localized_status_label(language))
                                    .small()
                                    .strong()
                                    .color(theme::TEXT_SECONDARY),
                            );
                        });
                    });
                }
                if compact {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(protocol_state.localized_status_label(language))
                                .small()
                                .strong()
                                .color(theme::TEXT_SECONDARY),
                        )
                        .truncate(),
                    );
                }
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(&protocol_summary)
                            .monospace()
                            .small()
                            .color(theme::TEXT_PRIMARY),
                    )
                    .truncate(),
                )
                .on_hover_text(&protocol_summary);
                if !compact && matches!(protocol_state, PowerProtocolState::TraditionalUnconfirmed) {
                    ui.label(
                        egui::RichText::new(format!(
                            "VBUS {:.2} V · D+ {} · D− {}",
                            current.voltage,
                            dp.map_or_else(|| "—".to_string(), |value| format!("{value:.2} V")),
                            dm.map_or_else(|| "—".to_string(), |value| format!("{value:.2} V")),
                        ))
                        .monospace()
                        .small()
                        .color(theme::MUTED_TEXT),
                    );
                }
            });
        if protocol_card
            .response
            .interact(egui::Sense::click())
            .on_hover_text(language.pick(
                "打开 PD 分析，查看 PDO、RDO 和完整协议时间线",
                "Open PD Analysis to inspect PDOs, the RDO, and the full protocol timeline",
            ))
            .clicked()
        {
            self.active_tab = WorkspaceTab::PdAnalysis;
        }

        ui.add_space(if compact { 8.0 } else { 12.0 });
        let signal_width = ui.available_width();
        egui::Frame::NONE
            .fill(theme::PANEL)
            .stroke(egui::Stroke::new(1.0, theme::DIVIDER))
            .corner_radius(egui::CornerRadius::same(6))
            .inner_margin(egui::Margin::symmetric(9, if compact { 5 } else { 7 }))
            .show(ui, |ui| {
                ui.set_min_width((signal_width - 18.0).max(120.0));
                if compact {
                    ui.horizontal(|ui| {
                        compact_signal_value(ui, "D+", dp, language);
                        compact_signal_value(ui, "D−", dm, language);
                        compact_signal_value(ui, "CC1", cc1, language);
                        compact_signal_value(ui, "CC2", cc2, language);
                    });
                } else {
                    ui.label(
                        egui::RichText::new(language.pick("信号线", "Signal lines"))
                            .strong()
                            .size(16.0),
                    );
                    egui::Grid::new("signal_grid")
                        .num_columns(2)
                        .spacing([18.0, 4.0])
                        .show(ui, |ui| {
                            let signal_chip_width = ((ui.available_width() - 18.0) / 2.0).max(72.0);
                            signal_value(ui, "D+", dp, signal_chip_width, language);
                            signal_value(ui, "D−", dm, signal_chip_width, language);
                            ui.end_row();
                            signal_value(ui, "CC1", cc1, signal_chip_width, language);
                            signal_value(ui, "CC2", cc2, signal_chip_width, language);
                            ui.end_row();
                        });
                }
            });
    }
}

/// Quiet settings surface. Navigation decides which small set of sections is
/// visible, so the content itself no longer needs nested accordion chrome.
fn settings_section(ui: &mut egui::Ui, title: &str, _default_open: bool, add_contents: impl FnOnce(&mut egui::Ui)) {
    let frame_width = ui.available_width();
    egui::Frame::NONE
        .fill(theme::PANEL_RAISED)
        .stroke(egui::Stroke::NONE)
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(14, 12))
        .show(ui, |ui| {
            let content_width = (frame_width - 28.0).max(120.0);
            ui.set_width(content_width);
            ui.set_max_width(content_width);
            ui.label(
                egui::RichText::new(title)
                    .strong()
                    .size(15.0)
                    .color(theme::TEXT_PRIMARY),
            );
            ui.add_space(8.0);
            add_contents(ui);
        });
    ui.add_space(10.0);
}

const SETTINGS_FORM_LABEL_WIDTH: f32 = 148.0;

fn settings_form_label(ui: &mut egui::Ui, label: &str) {
    ui.allocate_ui_with_layout(
        egui::vec2(SETTINGS_FORM_LABEL_WIDTH, ui.spacing().interact_size.y),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_width(SETTINGS_FORM_LABEL_WIDTH);
            ui.add(egui::Label::new(egui::RichText::new(label).color(theme::MUTED_TEXT)).wrap());
        },
    );
}

fn settings_control_width(available_width: f32) -> f32 {
    (available_width - SETTINGS_FORM_LABEL_WIDTH - 12.0).clamp(180.0, 360.0)
}

const RECOVERABLE_FILE_BUTTON_WIDTH: f32 = 72.0;
const RECOVERABLE_FILE_COLUMN_GAP: f32 = 10.0;
const RECOVERABLE_SESSION_CONTINUE_WIDTH: f32 = 100.0;
const RECOVERABLE_SESSION_SECONDARY_WIDTH: f32 = 64.0;
const RECOVERABLE_SESSION_ACTION_GAP: f32 = 8.0;

fn recoverable_file_columns(available_width: f32) -> (f32, f32) {
    (
        (available_width - RECOVERABLE_FILE_BUTTON_WIDTH - RECOVERABLE_FILE_COLUMN_GAP).max(120.0),
        RECOVERABLE_FILE_BUTTON_WIDTH,
    )
}

fn recoverable_session_columns(available_width: f32) -> (f32, f32) {
    let action_width = RECOVERABLE_SESSION_CONTINUE_WIDTH
        + RECOVERABLE_SESSION_SECONDARY_WIDTH * 2.0
        + RECOVERABLE_SESSION_ACTION_GAP * 2.0;
    ((available_width - action_width - 12.0).max(160.0), action_width)
}

const fn localized_session_state(state: SessionState, language: Language) -> &'static str {
    match state {
        SessionState::Recording => language.pick("录制中", "Recording"),
        SessionState::Paused => language.pick("已暂停", "Paused"),
        SessionState::WaitingForReconnect => language.pick("等待重连", "Waiting to reconnect"),
        SessionState::Finalizing => language.pick("正在保存", "Saving"),
        SessionState::Saved => language.pick("已保存", "Saved"),
        SessionState::Interrupted => language.pick("异常中断", "Interrupted"),
    }
}

struct InstrumentCardData<'a> {
    label: &'a str,
    value: f64,
    unit: MeasurementUnit,
    color: egui::Color32,
    statistics: Option<MetricStatistics>,
    compact: bool,
    language: Language,
    readout_status: &'a str,
}

fn instrument_card(ui: &mut egui::Ui, data: InstrumentCardData<'_>) {
    let InstrumentCardData {
        label,
        value,
        unit,
        color,
        statistics,
        compact,
        language,
        readout_status,
    } = data;
    let card_width = ui.available_width();
    let range_maximum = statistics.map_or(value.abs(), |statistics| {
        statistics.maximum.abs().max(statistics.minimum.abs()).max(value.abs())
    });
    let presentation = EngineeringPresentation::for_maximum(range_maximum, unit);
    let channel = match unit {
        MeasurementUnit::Voltage => "VBUS",
        MeasurementUnit::Current => "IBUS",
        MeasurementUnit::Power => "PWR",
    };
    let card = egui::Frame::NONE
        .fill(theme::PANEL_RAISED)
        .stroke(egui::Stroke::new(1.0, theme::DIVIDER))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(
            if compact { 12 } else { 16 },
            if compact { 7 } else { 10 },
        ))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.set_min_width((card_width - if compact { 24.0 } else { 32.0 }).max(120.0));
            ui.set_min_height(if compact { 78.0 } else { 96.0 });
            ui.horizontal(|ui| {
                let status_width = 58.0;
                let label_width = (ui.available_width() - status_width - 4.0).max(72.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(label_width, 20.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(format!("{label}  {channel}"))
                                    .color(theme::TEXT_PRIMARY)
                                    .strong()
                                    .size(16.0),
                            )
                            .truncate(),
                        );
                    },
                );
                ui.allocate_ui_with_layout(
                    egui::vec2(status_width, 20.0),
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        ui.label(egui::RichText::new(readout_status).small().color(theme::TEXT_SECONDARY));
                    },
                );
            });
            ui.add_space(if compact { 0.0 } else { 2.0 });
            ui.horizontal(|ui| {
                let unit_width = 38.0;
                let value_width = (ui.available_width() - unit_width - 4.0).max(48.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(value_width, if compact { 29.0 } else { 36.0 }),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.label(
                            egui::RichText::new(presentation.format_value(value))
                                .monospace()
                                .size(if compact { 30.0 } else { 34.0 })
                                .strong()
                                .color(color),
                        );
                    },
                );
                ui.allocate_ui_with_layout(
                    egui::vec2(unit_width, if compact { 29.0 } else { 36.0 }),
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        ui.label(
                            egui::RichText::new(presentation.symbol)
                                .monospace()
                                .size(14.0)
                                .strong()
                                .color(color),
                        );
                    },
                );
            });
            egui::Frame::NONE
                .fill(theme::PANEL)
                .corner_radius(egui::CornerRadius::same(5))
                .inner_margin(egui::Margin::symmetric(7, if compact { 2 } else { 3 }))
                .show(ui, |ui| {
                    if let Some(statistics) = statistics {
                        let values = [
                            presentation.format_value(statistics.minimum),
                            presentation.format_value(statistics.average),
                            presentation.format_value(statistics.maximum),
                        ];
                        ui.columns(3, |columns| {
                            for (index, heading) in [
                                language.pick("最小", if compact { "Min" } else { "Minimum" }),
                                language.pick("平均", if compact { "Avg" } else { "Average" }),
                                language.pick("最大", if compact { "Max" } else { "Maximum" }),
                            ]
                            .into_iter()
                            .enumerate()
                            {
                                columns[index].vertical_centered(|ui| {
                                    ui.spacing_mut().item_spacing.y = 0.0;
                                    ui.label(egui::RichText::new(heading).small().color(theme::TEXT_MUTED));
                                    ui.label(
                                        egui::RichText::new(&values[index])
                                            .monospace()
                                            .small()
                                            .color(theme::TEXT_PRIMARY),
                                    );
                                });
                            }
                        });
                    } else {
                        ui.vertical_centered(|ui| {
                            ui.set_min_height(if compact { 28.0 } else { 34.0 });
                            ui.label(
                                egui::RichText::new(language.pick(
                                    "最小 · 平均 · 最大",
                                    if compact {
                                        "Min · Avg · Max"
                                    } else {
                                        "Minimum · Average · Maximum"
                                    },
                                ))
                                .small()
                                .color(theme::TEXT_MUTED),
                            );
                            ui.label(
                                egui::RichText::new(
                                    language.pick("记录后显示统计", "Statistics appear after recording"),
                                )
                                .small()
                                .color(theme::TEXT_MUTED.gamma_multiply(0.72)),
                            );
                        });
                    }
                });
        });
    let stripe_rect = egui::Rect::from_min_max(
        egui::pos2(card.response.rect.left() + 1.0, card.response.rect.top() + 8.0),
        egui::pos2(card.response.rect.left() + 4.0, card.response.rect.bottom() - 8.0),
    );
    ui.painter().rect_filled(stripe_rect, 2.0, color);
}

fn signal_value(ui: &mut egui::Ui, label: &str, value: Option<f64>, width: f32, language: Language) {
    let response = ui.allocate_ui_with_layout(
        egui::vec2(width, ui.spacing().interact_size.y),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.label(egui::RichText::new(label).strong().color(theme::TEXT_SECONDARY));
            ui.label(
                egui::RichText::new(value.map_or_else(|| "—".to_string(), |value| format!("{value:.2} V")))
                    .monospace()
                    .color(theme::TEXT_PRIMARY),
            );
        },
    );
    let explanation = match label {
        "D+" | "D−" => language.pick(
            "USB 2.0 数据线电压，也可用于部分传统充电识别协议。",
            "USB 2.0 data-line voltage, also used by some legacy charging-detection protocols.",
        ),
        "CC1" | "CC2" => language.pick(
            "USB-C 配置通道：用于方向、角色、电流能力与 USB PD 协商。",
            "USB-C Configuration Channel used for orientation, roles, current advertisement, and USB PD negotiation.",
        ),
        _ => language.pick("KM003C 实时信号线电压。", "Live KM003C signal-line voltage."),
    };
    response.response.on_hover_text(explanation);
}

fn compact_signal_value(ui: &mut egui::Ui, label: &str, value: Option<f64>, language: Language) {
    let text = value.map_or_else(|| format!("{label} —"), |value| format!("{label} {value:.2}"));
    let response = ui.label(
        egui::RichText::new(text)
            .monospace()
            .small()
            .color(theme::TEXT_SECONDARY),
    );
    response.on_hover_text(match label {
        "D+" | "D−" => language.pick(
            "USB 2.0 数据线电压（V），也可用于部分传统充电识别协议。",
            "USB 2.0 data-line voltage (V), also used by some legacy charging-detection protocols.",
        ),
        "CC1" | "CC2" => language.pick(
            "USB-C 配置通道电压（V）：用于方向、角色、电流能力与 USB PD 协商。",
            "USB-C Configuration Channel voltage (V), used for orientation, roles, current advertisement, and USB PD negotiation.",
        ),
        _ => language.pick("KM003C 实时信号线电压（V）。", "Live KM003C signal-line voltage (V)."),
    });
}

impl PowerMonitorApp {
    fn show_imported_recording_banner(&mut self, ui: &mut egui::Ui, compact: bool) {
        if self.plot_source != PlotSource::Imported {
            return;
        }
        let language = self.language;
        let Some(recording) = &self.imported_recording else {
            return;
        };
        let file_name = recording
            .path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("KM003C recording")
            .to_string();
        let metadata = recording.metadata.clone();
        let points = recording.samples.len();
        let mut close_requested = false;
        egui::Frame::NONE
            .fill(theme::PANEL_RAISED)
            .stroke(egui::Stroke::new(1.0, theme::DIVIDER))
            .corner_radius(egui::CornerRadius::same(6))
            .inner_margin(egui::Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(file_name).strong().color(theme::TEXT_PRIMARY));
                    if !compact {
                        if let Some(metadata) = &metadata {
                            ui.separator();
                            ui.monospace(&metadata.timestamps.started_at_beijing);
                            ui.label("→");
                            ui.monospace(
                                metadata
                                    .timestamps
                                    .ended_at_beijing
                                    .as_deref()
                                    .unwrap_or(language.pick("结束时间未知", "End time unknown")),
                            );
                            ui.separator();
                            ui.monospace(format!(
                                "{} · {points} pts · {:.3}%",
                                format_recording_duration(Duration::from_micros(metadata.effective_duration_us)),
                                metadata.completeness_percent,
                            ));
                        } else {
                            ui.label(
                                egui::RichText::new(language.pick(
                                    "记录时间未知 · 旧文件没有会话元数据",
                                    "Recording time unknown · This legacy file has no session metadata",
                                ))
                                .small()
                                .color(theme::MUTED_TEXT),
                            );
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        close_requested = ui
                            .button(if compact {
                                language.pick("关闭", "Close")
                            } else {
                                language.pick("关闭导入文件", "Close imported file")
                            })
                            .clicked();
                    });
                });
            });
        ui.add_space(6.0);
        if close_requested {
            self.close_imported_recording();
        }
    }

    fn show_combined_monitor_chart(&mut self, ui: &mut egui::Ui, compact: bool) {
        let language = self.language;
        let selection = self.ensure_chart_selection();
        let navigator_height = if compact { 62.0 } else { 76.0 };
        let metadata_banner_height = if self.plot_source == PlotSource::Imported {
            44.0
        } else {
            0.0
        };
        let chart_height =
            (ui.available_height() - navigator_height - metadata_banner_height - if compact { 208.0 } else { 220.0 })
                .max(240.0);
        let max_plot_points = (ui.available_width().max(320.0) * 2.0) as usize;
        let vip_points = self.source_vip_points(selection, max_plot_points, self.display_filter);
        let raw_vip_points = (self.display_filter != DisplayFilter::Raw)
            .then(|| self.source_vip_points(selection, max_plot_points / 2, DisplayFilter::Raw));
        let scales = [
            AxisScale::from_visible_max(vip_points[0].iter().map(|point| point[1]).fold(0.0_f64, f64::max)),
            AxisScale::from_visible_max(vip_points[1].iter().map(|point| point[1]).fold(0.0_f64, f64::max)),
            AxisScale::from_visible_max(vip_points[2].iter().map(|point| point[1]).fold(0.0_f64, f64::max)),
        ];

        egui::Frame::NONE
            .fill(theme::PANEL)
            .stroke(egui::Stroke::new(1.0, theme::DIVIDER))
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                self.show_imported_recording_banner(ui, compact);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(language.pick("V / A / W 实时轨迹", "V / A / W live traces"))
                            .strong()
                            .size(16.0),
                    );
                    if !compact {
                        ui.label(
                            egui::RichText::new(self.source_label())
                                .small()
                                .color(theme::MUTED_TEXT),
                        );
                    }

                    if matches!(
                        self.recording_phase,
                        RecordingPhase::Recording
                            | RecordingPhase::Paused
                            | RecordingPhase::Interrupted
                            | RecordingPhase::WaitingForReconnect
                            | RecordingPhase::Recovering
                    ) {
                        let (color, text) = match self.recording_phase {
                            RecordingPhase::Paused => (
                                theme::TEXT_SECONDARY,
                                if compact {
                                    language.pick("Ⅱ 录制暂停", "Ⅱ Paused")
                                } else {
                                    language.pick(
                                        "Ⅱ 录制已暂停 · 设备仍在采样",
                                        "Ⅱ Recording paused · Device still sampling",
                                    )
                                },
                            ),
                            RecordingPhase::WaitingForReconnect => (
                                theme::POWER,
                                language.pick("USB 中断 · 等待重连", "USB interrupted · Waiting to reconnect"),
                            ),
                            RecordingPhase::Recovering => (
                                theme::POWER,
                                language.pick("正在恢复续录", "Restoring recording"),
                            ),
                            RecordingPhase::Interrupted => (
                                theme::RECORDING,
                                language.pick("录制中断 · 可恢复", "Recording interrupted · Recoverable"),
                            ),
                            _ => (
                                theme::RECORDING,
                                if compact {
                                    language.pick("● 录制中", "● Recording")
                                } else {
                                    language.pick("● 正在录制", "● Recording in progress")
                                },
                            ),
                        };
                        egui::Frame::NONE
                            .fill(theme::PANEL_RAISED)
                            .stroke(egui::Stroke::new(1.0, theme::DIVIDER))
                            .corner_radius(egui::CornerRadius::same(6))
                            .inner_margin(egui::Margin::symmetric(7, 3))
                            .show(ui, |ui| {
                                ui.colored_label(color, egui::RichText::new(text).strong().small());
                            });
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let range_label = match self.chart_follow_mode {
                            ChartFollowMode::FullSession => language.pick("全程", "Full session").to_string(),
                            ChartFollowMode::LatestWindow => format!(
                                "{} {}",
                                language.pick("最近", "Latest"),
                                self.time_window.localized_label(language)
                            ),
                            ChartFollowMode::Manual => language.pick("手动窗口", "Manual window").to_string(),
                        };
                        egui::ComboBox::from_id_salt("monitor_range_mode")
                            .width(if compact { 96.0 } else { 132.0 })
                            .selected_text(range_label)
                            .show_ui(ui, |ui| {
                                if ui
                                    .selectable_label(
                                        self.chart_follow_mode == ChartFollowMode::FullSession,
                                        language.pick("全程 · 0 到最新", "Full session · 0 to latest"),
                                    )
                                    .clicked()
                                {
                                    self.time_window = TimeWindow::All;
                                    self.chart_follow_mode = ChartFollowMode::FullSession;
                                    self.chart_viewport.selection = None;
                                }
                                for window in TimeWindow::all()
                                    .iter()
                                    .copied()
                                    .filter(|window| *window != TimeWindow::All)
                                {
                                    if ui
                                        .selectable_label(
                                            self.chart_follow_mode == ChartFollowMode::LatestWindow
                                                && self.time_window == window,
                                            format!(
                                                "{} {}",
                                                language.pick("跟随最近", "Follow latest"),
                                                window.localized_label(language)
                                            ),
                                        )
                                        .clicked()
                                    {
                                        self.time_window = window;
                                        self.chart_follow_mode = ChartFollowMode::LatestWindow;
                                        self.chart_viewport.selection = None;
                                    }
                                }
                                ui.add_enabled_ui(self.chart_follow_mode == ChartFollowMode::Manual, |ui| {
                                    let _ = ui.selectable_label(
                                        true,
                                        language.pick("手动窗口 · 拖动中", "Manual window · Dragging"),
                                    );
                                });
                            })
                            .response
                            .on_hover_text(
                                language.pick(
                                    "全程会从 00:00:00.0 展开；最近窗口会保持指定宽度；拖动或缩放后进入手动窗口",
                                    "Full session grows from 00:00:00.0. Latest keeps a fixed-width window. Drag or zoom to enter manual view.",
                                ),
                            );
                        if self.chart_follow_mode == ChartFollowMode::Manual
                            && ui
                                .button(if compact {
                                    language.pick("最新", "Latest")
                                } else {
                                    language.pick("回到最新", "Back to latest")
                                })
                                .clicked()
                        {
                            if self.time_window == TimeWindow::All {
                                self.time_window = TimeWindow::Sec30;
                            }
                            self.chart_follow_mode = ChartFollowMode::LatestWindow;
                            self.chart_viewport.selection = None;
                        }
                    });
                });

                ui.horizontal(|ui| {
                    for (series_index, (label, color, unit)) in [
                        (language.pick("电压", "Voltage"), theme::VOLTAGE, MeasurementUnit::Voltage),
                        (language.pick("电流", "Current"), theme::CURRENT, MeasurementUnit::Current),
                        (language.pick("功率", "Power"), theme::POWER, MeasurementUnit::Power),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        let visible = self.visible_series[series_index];
                        let presentation = scales[series_index].presentation(unit);
                        let marker = if series_index == 2 { "┄" } else { "●" };
                        let text = if compact {
                            format!("{marker} {label}")
                        } else {
                            format!(
                                "{marker} {label}  0–{} {}",
                                presentation.format_value(scales[series_index].maximum),
                                presentation.symbol
                            )
                        };
                        let button = egui::Button::new(egui::RichText::new(text).color(if visible {
                            color
                        } else {
                            theme::TEXT_MUTED
                        }))
                        .fill(theme::PANEL_RAISED)
                        .stroke(egui::Stroke::new(1.0, theme::DIVIDER))
                        .corner_radius(egui::CornerRadius::same(6))
                        .min_size(egui::vec2(if compact { 76.0 } else { 128.0 }, 28.0));
                        if ui
                            .add(button)
                            .on_hover_text(if visible {
                                language.pick("点击隐藏曲线", "Hide trace")
                            } else {
                                language.pick("点击显示曲线", "Show trace")
                            })
                            .clicked()
                        {
                            self.visible_series[series_index] = !visible;
                            if !self.visible_series.iter().any(|visible| *visible) {
                                self.visible_series[series_index] = true;
                            }
                        }
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let pin_label = if self.cursor_pinned {
                            if compact {
                                language.pick("取消固定", "Unpin")
                            } else {
                                language.pick("取消固定游标", "Unpin cursor")
                            }
                        } else if compact {
                            language.pick("固定", "Pin")
                        } else {
                            language.pick("固定游标", "Pin cursor")
                        };
                        if ui
                            .add_enabled(
                                self.cursor_readout.is_some(),
                                egui::Button::new(pin_label).selected(self.cursor_pinned),
                            )
                            .clicked()
                        {
                            self.cursor_pinned = !self.cursor_pinned;
                        }
                    });
                });

                ui.add_space(4.0);
                self.show_cursor_readout_strip(ui, self.cursor_readout);
                ui.add_space(4.0);

                let visible_series = self.visible_series;
                let mut axes = Vec::with_capacity(3);
                for (series_index, (label, color, unit, placement)) in [
                    (
                        language.pick("电压", "Voltage"),
                        theme::VOLTAGE,
                        MeasurementUnit::Voltage,
                        HPlacement::Left,
                    ),
                    (
                        language.pick("电流", "Current"),
                        theme::CURRENT,
                        MeasurementUnit::Current,
                        HPlacement::Left,
                    ),
                    (
                        language.pick("功率", "Power"),
                        theme::POWER,
                        MeasurementUnit::Power,
                        HPlacement::Right,
                    ),
                ]
                .into_iter()
                .enumerate()
                {
                    if !visible_series[series_index] {
                        continue;
                    }
                    let scale = scales[series_index];
                    let presentation = scale.presentation(unit);
                    let axis_label = if compact {
                        presentation.symbol.to_string()
                    } else {
                        format!("{label} ({})", presentation.symbol)
                    };
                    axes.push(
                        AxisHints::new_y()
                            .label(egui::RichText::new(axis_label).color(color))
                            .placement(placement)
                            .tick_label_color(color)
                            .min_thickness(if compact { 38.0 } else { 52.0 })
                            .formatter(move |mark: GridMark, _| {
                                presentation.format_value(mark.value * scale.maximum)
                            }),
                    );
                }
                let x_axis = AxisHints::new_x()
                    .formatter(|mark: GridMark, _| format_plot_time(mark.value))
                    .tick_label_color(theme::TEXT_SECONDARY)
                    .min_thickness(28.0);
                let normalized_points = [
                    vip_points[0]
                        .iter()
                        .map(|point| [point[0], scales[0].normalize(point[1])])
                        .collect::<Vec<_>>(),
                    vip_points[1]
                        .iter()
                        .map(|point| [point[0], scales[1].normalize(point[1])])
                        .collect::<Vec<_>>(),
                    vip_points[2]
                        .iter()
                        .map(|point| [point[0], scales[2].normalize(point[1])])
                        .collect::<Vec<_>>(),
                ];
                let normalized_raw_points = raw_vip_points.as_ref().map(|points| {
                    [
                        points[0]
                            .iter()
                            .map(|point| [point[0], scales[0].normalize(point[1])])
                            .collect::<Vec<_>>(),
                        points[1]
                            .iter()
                            .map(|point| [point[0], scales[1].normalize(point[1])])
                            .collect::<Vec<_>>(),
                        points[2]
                            .iter()
                            .map(|point| [point[0], scales[2].normalize(point[1])])
                            .collect::<Vec<_>>(),
                    ]
                });
                let pause_intervals = self.pause_intervals.clone();
                let active_pause = self.active_pause_started_at;
                let active_pause_end = self.source_end_time();
                let pinned_cursor = self.cursor_pinned.then_some(self.cursor_readout).flatten();
                let plot_response = Plot::new("combined_monitor_plot")
                    .height(chart_height)
                    .custom_x_axes(vec![x_axis])
                    .custom_y_axes(axes)
                    .show_grid([true, true])
                    .grid_color(theme::DIVIDER.gamma_multiply(0.52))
                    .grid_fade(0.85)
                    .show_crosshair(false)
                    .allow_boxed_zoom(false)
                    .allow_drag(false)
                    .allow_scroll(false)
                    .allow_zoom(false)
                    .default_x_bounds(
                        selection.start_seconds,
                        selection.end_seconds.max(selection.start_seconds + 0.001),
                    )
                    .default_y_bounds(0.0, 1.0)
                    .auto_bounds([false, false])
                    .show(ui, |plot_ui| {
                        plot_ui.set_plot_bounds(PlotBounds::from_min_max(
                            [selection.start_seconds, 0.0],
                            [selection.end_seconds.max(selection.start_seconds + 0.001), 1.0],
                        ));
                        for (index, interval) in pause_intervals.iter().enumerate() {
                            plot_ui.span(
                                Span::new(
                                    format!("{} {index}", language.pick("暂停区间", "Pause interval")),
                                    interval.start_seconds..=interval.end_seconds,
                                )
                                .fill(egui::Color32::from_rgba_unmultiplied(140, 148, 158, 28))
                                .border(egui::Stroke::new(1.0, theme::MUTED_TEXT.gamma_multiply(0.55)))
                                .border_style(LineStyle::dashed_dense()),
                            );
                        }
                        if let Some(start) = active_pause
                            && active_pause_end >= start
                        {
                            plot_ui.span(
                                Span::new(
                                    language.pick("已暂停 · 未写入录制", "Paused · Excluded from recording"),
                                    start..=active_pause_end,
                                )
                                    .fill(egui::Color32::from_rgba_unmultiplied(140, 148, 158, 35))
                                    .border(egui::Stroke::new(1.0, theme::POWER.gamma_multiply(0.7)))
                                    .border_style(LineStyle::dashed_dense()),
                            );
                        }

                        if let Some(raw_points) = &normalized_raw_points {
                            for (index, (name, color)) in [
                                (language.pick("电压原始包络", "Raw voltage envelope"), theme::VOLTAGE),
                                (language.pick("电流原始包络", "Raw current envelope"), theme::CURRENT),
                                (language.pick("功率原始包络", "Raw power envelope"), theme::POWER),
                            ]
                            .into_iter()
                            .enumerate()
                            {
                                if visible_series[index] {
                                    plot_ui.line(
                                        Line::new(name, PlotPoints::from(raw_points[index].clone()))
                                            .color(color.gamma_multiply(0.14))
                                            .width(0.7),
                                    );
                                }
                            }
                        }

                        if visible_series[0] {
                            plot_ui.line(
                                Line::new(
                                    language.pick("电压", "Voltage"),
                                    PlotPoints::from(normalized_points[0].clone()),
                                )
                                    .color(theme::VOLTAGE)
                                    .width(1.8),
                            );
                        }
                        if visible_series[1] {
                            plot_ui.line(
                                Line::new(
                                    language.pick("电流", "Current"),
                                    PlotPoints::from(normalized_points[1].clone()),
                                )
                                    .color(theme::CURRENT)
                                    .width(1.8),
                            );
                        }
                        if visible_series[2] {
                            plot_ui.line(
                                Line::new(
                                    language.pick("功率", "Power"),
                                    PlotPoints::from(normalized_points[2].clone()),
                                )
                                    .color(theme::POWER)
                                    .width(1.7)
                                    .style(LineStyle::dashed_dense()),
                            );
                        }

                        let readout = if let Some(readout) = pinned_cursor {
                            readout
                        } else {
                            let hovered_time = plot_ui
                                .response()
                                .hovered()
                                .then(|| plot_ui.pointer_coordinate().map(|point| point.x))
                                .flatten()?;
                            self.cursor_readout_at(hovered_time)?
                        };
                        plot_ui.vline(
                            VLine::new(language.pick("联动游标", "Linked cursor"), readout.time_seconds)
                                .color(theme::TEXT_SECONDARY)
                                .width(1.0)
                                .style(LineStyle::dashed_dense()),
                        );
                        for (index, (value, scale, color)) in [
                            (readout.voltage, scales[0], theme::VOLTAGE),
                            (readout.current, scales[1], theme::CURRENT),
                            (readout.power, scales[2], theme::POWER),
                        ]
                        .into_iter()
                        .enumerate()
                        {
                            if visible_series[index] {
                                plot_ui.points(
                                    Points::new(
                                        format!("{} {index}", language.pick("游标点", "Cursor point")),
                                        vec![[readout.time_seconds, scale.normalize(value)]],
                                    )
                                    .color(color)
                                    .filled(true)
                                    .radius(4.0),
                                );
                            }
                        }
                        Some(readout)
                    });

                plot_response.response.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::WidgetType::Other,
                        true,
                        language.pick(
                            "电压、电流和功率联动曲线；移动鼠标读取同一时刻数值",
                            "Linked voltage, current, and power trace. Move the pointer to inspect matching values.",
                        ),
                    )
                });

                if plot_response.response.dragged() {
                    let delta = ui.ctx().input(|input| input.pointer.delta().x);
                    let frame_width = plot_response.transform.frame().width().max(1.0);
                    let shift = -(delta as f64 / frame_width as f64) * selection.width();
                    self.chart_viewport.selection = Some(
                        NavigatorSelection {
                            start_seconds: selection.start_seconds + shift,
                            end_seconds: selection.end_seconds + shift,
                        }
                        .clamped(self.source_end_time()),
                    );
                    self.enter_manual_chart_view();
                }
                if plot_response.response.hovered() {
                    let scroll = ui.ctx().input(|input| input.smooth_scroll_delta.y);
                    if scroll.abs() > f32::EPSILON {
                        let factor = (f64::from(scroll) * 0.006).exp();
                        let full_end = self.source_end_time().max(0.001);
                        let new_width = (selection.width() / factor).clamp(0.05, full_end);
                        let anchor = plot_response
                            .response
                            .hover_pos()
                            .map(|position| plot_response.transform.value_from_position(position).x)
                            .unwrap_or((selection.start_seconds + selection.end_seconds) * 0.5);
                        let fraction = ((anchor - selection.start_seconds) / selection.width()).clamp(0.0, 1.0);
                        self.chart_viewport.selection = Some(
                            NavigatorSelection {
                                start_seconds: anchor - new_width * fraction,
                                end_seconds: anchor + new_width * (1.0 - fraction),
                            }
                            .clamped(full_end),
                        );
                        self.enter_manual_chart_view();
                    }
                }
                if let Some(readout) = plot_response.inner
                    && !self.cursor_pinned
                {
                    self.cursor_readout = Some(readout);
                }

                ui.add_space(8.0);
                self.show_scope_statistics_bar(ui, selection, compact);
            });

        ui.add_space(8.0);
        self.show_time_navigator(ui, navigator_height);
        self.reset_plots_requested = false;
    }

    fn show_scope_statistics_bar(&self, ui: &mut egui::Ui, selection: NavigatorSelection, compact: bool) {
        let language = self.language;
        let all = self.full_scope_statistics();
        let window = self.window_scope_statistics(selection);
        let energy = EnergyPresentation::for_values([all.cumulative_energy_uwh, window.cumulative_energy_uwh]);
        let width = ui.available_width();
        egui::Frame::NONE
            .fill(theme::PANEL_RAISED)
            .stroke(egui::Stroke::new(1.0, theme::DIVIDER))
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::symmetric(if compact { 8 } else { 12 }, 5))
            .show(ui, |ui| {
                ui.set_min_width((width - if compact { 16.0 } else { 24.0 }).max(240.0));
                egui::Grid::new("scope_statistics_grid")
                    .num_columns(5)
                    .spacing([if compact { 12.0 } else { 22.0 }, 2.0])
                    .striped(false)
                    .show(ui, |ui| {
                        for heading in [
                            language.pick("记录范围", "Scope"),
                            language.pick("时长", "Duration"),
                            language.pick("容量", "Capacity"),
                            language.pick("累计能量", "Energy"),
                            language.pick("点数", "Points"),
                        ] {
                            ui.label(egui::RichText::new(heading).small().color(theme::TEXT_MUTED));
                        }
                        ui.end_row();
                        for (name, statistics) in [
                            (language.pick("全部", "All"), all),
                            (language.pick("窗口", "Window"), window),
                        ] {
                            ui.label(
                                egui::RichText::new(if statistics.approximate {
                                    format!("≈{name}")
                                } else {
                                    name.to_string()
                                })
                                .strong()
                                .color(theme::TEXT_PRIMARY),
                            );
                            ui.monospace(format_plot_time(statistics.duration_seconds));
                            ui.monospace(format_capacity(statistics.capacity_uah));
                            ui.monospace(energy.format(statistics.cumulative_energy_uwh));
                            ui.monospace(statistics.points.to_string());
                            ui.end_row();
                        }
                    });
            });
    }

    fn show_time_navigator(&mut self, ui: &mut egui::Ui, height: f32) {
        let language = self.language;
        let full_end = self.source_end_time().max(0.001);
        let mut selection = self.chart_viewport.selection.unwrap_or(NavigatorSelection {
            start_seconds: 0.0,
            end_seconds: full_end,
        });
        let desired_size = egui::vec2(ui.available_width(), height);
        let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::click_and_drag());
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Slider,
                true,
                language.pick(
                    "全程时间导航；拖动选区或两端改变显示范围，双击恢复全程",
                    "Full-session navigator. Drag the selection or either handle to change the range; double-click for the full session.",
                ),
            )
        });
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 7.0, theme::PANEL);
        painter.rect_stroke(
            rect,
            7.0,
            egui::Stroke::new(1.0, theme::DIVIDER),
            egui::StrokeKind::Inside,
        );
        let label_height = 22.0;
        let graph_rect = egui::Rect::from_min_max(
            egui::pos2(rect.left() + 10.0, rect.top() + label_height),
            egui::pos2(rect.right() - 10.0, rect.bottom() - 8.0),
        );
        let time_to_x = |time: f64| graph_rect.left() + (time / full_end) as f32 * graph_rect.width();
        let x_to_time =
            |x: f32| (f64::from((x - graph_rect.left()) / graph_rect.width().max(1.0)) * full_end).clamp(0.0, full_end);
        let overview = self.navigator_vip_points((graph_rect.width() * 2.0) as usize);
        for (index, color) in [theme::VOLTAGE, theme::CURRENT, theme::POWER].into_iter().enumerate() {
            let maximum = overview[index]
                .iter()
                .map(|point| point[1])
                .fold(0.0_f64, f64::max)
                .max(f64::EPSILON);
            let screen_points = overview[index]
                .iter()
                .map(|point| {
                    egui::pos2(
                        time_to_x(point[0]),
                        graph_rect.bottom() - (point[1] / maximum) as f32 * graph_rect.height(),
                    )
                })
                .collect::<Vec<_>>();
            if screen_points.len() > 1 {
                if index == 2 {
                    let mut fill = Vec::with_capacity(screen_points.len() + 2);
                    fill.push(egui::pos2(screen_points[0].x, graph_rect.bottom()));
                    fill.extend(screen_points.iter().copied());
                    fill.push(egui::pos2(screen_points.last().unwrap().x, graph_rect.bottom()));
                    painter.add(egui::Shape::convex_polygon(
                        fill,
                        theme::POWER.gamma_multiply(0.09),
                        egui::Stroke::NONE,
                    ));
                }
                painter.add(egui::Shape::line(
                    screen_points,
                    egui::Stroke::new(if index == 2 { 1.2 } else { 0.9 }, color.gamma_multiply(0.78)),
                ));
            }
        }

        let start_x = time_to_x(selection.start_seconds);
        let end_x = time_to_x(selection.end_seconds);
        let selected_rect = egui::Rect::from_min_max(
            egui::pos2(start_x, graph_rect.top()),
            egui::pos2(end_x, graph_rect.bottom()),
        );
        let outside_mask = egui::Color32::from_rgba_unmultiplied(2, 5, 8, 118);
        if start_x > graph_rect.left() {
            painter.rect_filled(
                egui::Rect::from_min_max(graph_rect.left_top(), egui::pos2(start_x, graph_rect.bottom())),
                2.0,
                outside_mask,
            );
        }
        if end_x < graph_rect.right() {
            painter.rect_filled(
                egui::Rect::from_min_max(egui::pos2(end_x, graph_rect.top()), graph_rect.right_bottom()),
                2.0,
                outside_mask,
            );
        }
        painter.rect_filled(
            selected_rect,
            3.0,
            egui::Color32::from_rgba_unmultiplied(225, 240, 255, 24),
        );
        painter.rect_stroke(
            selected_rect,
            3.0,
            egui::Stroke::new(1.2, egui::Color32::from_rgb(0xC9, 0xDF, 0xF0)),
            egui::StrokeKind::Inside,
        );
        for x in [start_x, end_x] {
            painter.circle_filled(egui::pos2(x, graph_rect.center().y), 8.0, theme::PANEL_RAISED);
            painter.circle_stroke(
                egui::pos2(x, graph_rect.center().y),
                8.0,
                egui::Stroke::new(1.5, theme::VOLTAGE),
            );
        }
        painter.text(
            egui::pos2(graph_rect.left(), rect.top() + 4.0),
            egui::Align2::LEFT_TOP,
            format_plot_time(0.0),
            egui::FontId::monospace(12.0),
            theme::MUTED_TEXT,
        );
        let center_label = match self.chart_follow_mode {
            ChartFollowMode::FullSession => format!(
                "{} {} · {}",
                language.pick("全程", "Full"),
                format_plot_time(full_end),
                language.pick("已跟随最新", "Following latest")
            ),
            ChartFollowMode::LatestWindow => format!(
                "{} {} · {} {} · {}",
                language.pick("全程", "Full"),
                format_plot_time(full_end),
                language.pick("最近", "Latest"),
                format_plot_time(selection.width()),
                language.pick("已跟随最新", "Following latest")
            ),
            ChartFollowMode::Manual => {
                let lag = (full_end - selection.end_seconds).max(0.0);
                format!(
                    "{} {} · {} {} · {} {}",
                    language.pick("全程", "Full"),
                    format_plot_time(full_end),
                    language.pick("视窗", "Window"),
                    format_plot_time(selection.width()),
                    language.pick("距最新", "Behind latest"),
                    format_plot_time(lag)
                )
            }
        };
        painter.text(
            egui::pos2(graph_rect.center().x, rect.top() + 4.0),
            egui::Align2::CENTER_TOP,
            center_label,
            egui::FontId::monospace(12.0),
            egui::Color32::from_rgb(0xCF, 0xD8, 0xE2),
        );
        painter.text(
            egui::pos2(graph_rect.right(), rect.top() + 4.0),
            egui::Align2::RIGHT_TOP,
            format_plot_time(full_end),
            egui::FontId::monospace(12.0),
            theme::MUTED_TEXT,
        );

        if response.double_clicked() {
            selection = NavigatorSelection {
                start_seconds: 0.0,
                end_seconds: full_end,
            };
            self.chart_follow_mode = ChartFollowMode::FullSession;
            self.chart_viewport.drag = None;
        } else {
            if response.drag_started()
                && let Some(position) = response.interact_pointer_pos()
            {
                self.chart_viewport.drag = Some(if (position.x - start_x).abs() <= 12.0 {
                    NavigatorDrag::Start
                } else if (position.x - end_x).abs() <= 12.0 {
                    NavigatorDrag::End
                } else {
                    NavigatorDrag::Range
                });
                self.enter_manual_chart_view();
            }
            if response.dragged()
                && let Some(position) = response.interact_pointer_pos()
            {
                let minimum_width = (1.0 / self.current_rate.to_graph_rate().frequency().value).max(0.05);
                match self.chart_viewport.drag.unwrap_or(NavigatorDrag::Range) {
                    NavigatorDrag::Start => {
                        selection.start_seconds = x_to_time(position.x).min(selection.end_seconds - minimum_width);
                    }
                    NavigatorDrag::End => {
                        selection.end_seconds = x_to_time(position.x).max(selection.start_seconds + minimum_width);
                    }
                    NavigatorDrag::Range => {
                        let delta = ui.ctx().input(|input| input.pointer.delta().x);
                        let shift = f64::from(delta / graph_rect.width().max(1.0)) * full_end;
                        selection.start_seconds += shift;
                        selection.end_seconds += shift;
                    }
                }
                selection = selection.clamped(full_end);
            }
            if response.drag_stopped() {
                self.chart_viewport.drag = None;
            }
            if response.hovered() {
                let scroll = ui.ctx().input(|input| input.smooth_scroll_delta.y);
                if scroll.abs() > f32::EPSILON {
                    let anchor = response
                        .interact_pointer_pos()
                        .map_or((selection.start_seconds + selection.end_seconds) * 0.5, |position| {
                            x_to_time(position.x)
                        });
                    let factor = (f64::from(scroll) * 0.006).exp();
                    let new_width = (selection.width() / factor).clamp(0.05, full_end);
                    let fraction = ((anchor - selection.start_seconds) / selection.width()).clamp(0.0, 1.0);
                    selection = NavigatorSelection {
                        start_seconds: anchor - new_width * fraction,
                        end_seconds: anchor + new_width * (1.0 - fraction),
                    }
                    .clamped(full_end);
                    self.enter_manual_chart_view();
                }
            }
        }
        self.chart_viewport.selection = Some(selection.clamped(full_end));
    }
}

impl PowerMonitorApp {
    fn show_pd_analysis_page(&mut self, ui: &mut egui::Ui) {
        let language = self.language;
        let protocol_state = self.displayed_protocol_state();
        egui::Panel::left("pd_analysis_filters")
            .resizable(false)
            .exact_size(264.0)
            .frame(
                egui::Frame::NONE
                    .fill(egui::Color32::from_rgb(0x11, 0x16, 0x1C))
                    .stroke(egui::Stroke::new(1.0, theme::DIVIDER))
                    .inner_margin(egui::Margin::symmetric(12, 12)),
            )
            .show(ui, |ui| {
                ui.heading(language.pick("PD 分析", "PD Analysis"));
                ui.label(
                    egui::RichText::new(language.pick(
                        "KM003C 协议报文与固件状态",
                        "KM003C protocol messages and firmware state",
                    ))
                    .color(theme::MUTED_TEXT),
                );
                ui.add_space(10.0);

                let protocol_color = match protocol_state {
                    PowerProtocolState::Confirmed(_) => theme::CURRENT,
                    PowerProtocolState::PdDetected
                    | PowerProtocolState::Negotiating(_)
                    | PowerProtocolState::Waiting => theme::POWER,
                    PowerProtocolState::Disconnected
                    | PowerProtocolState::Unavailable
                    | PowerProtocolState::TraditionalUnconfirmed => theme::MUTED_TEXT,
                };
                egui::Frame::NONE
                    .fill(protocol_color.gamma_multiply(0.09))
                    .stroke(egui::Stroke::new(1.0, protocol_color.gamma_multiply(0.45)))
                    .corner_radius(egui::CornerRadius::same(6))
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(language.pick("当前充电协议", "Active charging protocol"))
                                    .strong(),
                            );
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.colored_label(
                                    protocol_color,
                                    protocol_state.localized_status_label(language),
                                );
                            });
                        });
                        let detail = protocol_state.contract().map_or_else(
                            || match protocol_state {
                                PowerProtocolState::PdDetected => {
                                    language
                                        .pick(
                                            "已收到 Source Capabilities，但合同尚未完成",
                                            "Source Capabilities received, but the power contract is not complete",
                                        )
                                        .to_string()
                                }
                                PowerProtocolState::TraditionalUnconfirmed => {
                                    language
                                        .pick(
                                            "未捕获完整 PD 协商；不根据电压猜测 QC、VOOC 或 UFCS",
                                            "A complete PD negotiation was not captured; QC, VOOC, and UFCS are not inferred from voltage",
                                        )
                                        .to_string()
                                }
                                PowerProtocolState::Waiting => language
                                    .pick(
                                        "等待 Source Capabilities / Request",
                                        "Waiting for Source Capabilities / Request",
                                    )
                                    .to_string(),
                                PowerProtocolState::Disconnected => language
                                    .pick(
                                        "请在 KM003C 受测端接入设备",
                                        "Connect a device to the KM003C test port",
                                    )
                                    .to_string(),
                                PowerProtocolState::Unavailable => language
                                    .pick(
                                        "当前离线数据不包含类型化 PD 合同",
                                        "The current offline data does not include a typed PD contract",
                                    )
                                    .to_string(),
                                PowerProtocolState::Negotiating(_) | PowerProtocolState::Confirmed(_) => unreachable!(),
                            },
                            |contract| contract.localized_summary(language),
                        );
                        ui.label(egui::RichText::new(detail).monospace().small());
                        if matches!(protocol_state, PowerProtocolState::Confirmed(_)) {
                            ui.label(
                                egui::RichText::new("Source Capabilities → Request → Accept → PS_RDY")
                                    .small()
                                    .color(theme::MUTED_TEXT),
                            );
                        }
                    });

                ui.add_space(10.0);

                egui::Frame::NONE
                    .fill(theme::PANEL_RAISED)
                    .stroke(egui::Stroke::new(1.0, theme::DIVIDER))
                    .corner_radius(egui::CornerRadius::same(6))
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new(language.pick("Type-C 状态", "Type-C status")).strong());
                        if let Some(pd) = &self.pd_status {
                            egui::Grid::new("pd_analysis_status_grid")
                                .num_columns(2)
                                .spacing([12.0, 5.0])
                                .show(ui, |ui| {
                                    ui.label("CC1");
                                    ui.monospace(format!("{:.3} V", pd.cc1.get::<volt>()));
                                    ui.end_row();
                                    ui.label("CC2");
                                    ui.monospace(format!("{:.3} V", pd.cc2.get::<volt>()));
                                    ui.end_row();
                                    ui.label(language.pick("连接", "Connection"));
                                    let (color, text) = match self.pd_connection.connected() {
                                        Some(true) => (theme::CURRENT, language.pick("已连接", "Connected")),
                                        Some(false) => (theme::RECORDING, language.pick("未连接", "Disconnected")),
                                        None => (theme::POWER, language.pick("检测中", "Detecting")),
                                    };
                                    ui.colored_label(color, text);
                                    ui.end_row();
                                });
                        } else {
                            ui.label(language.pick("尚未收到 PD 状态", "No PD status received yet"));
                        }
                    });

                ui.add_space(12.0);
                ui.label(egui::RichText::new(language.pick("时间线来源", "Timeline sources")).strong());
                ui.checkbox(
                    &mut self.pd_protocol_visible,
                    language.pick("WIRE 协议报文", "WIRE protocol messages"),
                );
                let trace_changed = ui
                    .checkbox(
                        &mut self.pd_trace_enabled,
                        language.pick("FW 固件 trace", "FW firmware trace"),
                    )
                    .changed();
                if trace_changed && self.device_state.is_some() {
                    let _ = self
                        .cmd_sender
                        .send(UsbCommand::SetPdTraceEnabled(self.pd_trace_enabled));
                }
                ui.checkbox(
                    &mut self.pd_auto_scroll,
                    language.pick("自动滚动到最新", "Auto-scroll to latest"),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button(language.pick("清空时间线", "Clear timeline")).clicked() {
                        self.clear_pd_log();
                    }
                    if ui.button(language.pick("返回监控", "Back to Monitor")).clicked() {
                        self.active_tab = WorkspaceTab::Monitor;
                    }
                });
                ui.add_space(10.0);
                ui.separator();
                ui.monospace(format!(
                    "WIRE  {} {}",
                    self.pd_log.len(),
                    language.pick("条", "entries")
                ));
                ui.monospace(format!(
                    "FW    {} {}",
                    self.pd_trace_log.len(),
                    language.pick("条", "entries")
                ));
                if self.pd_trace_enabled {
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(language.pick(
                            "FW 时间戳精度为 1 秒，同秒内先后顺序为近似值。",
                            "FW timestamps have 1-second precision; ordering within the same second is approximate.",
                        ))
                            .small()
                            .color(theme::MUTED_TEXT),
                    );
                }
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(theme::BACKPLANE)
                    .inner_margin(egui::Margin::symmetric(12, 12)),
            )
            .show(ui, |ui| {
                egui::Frame::NONE
                    .fill(theme::PANEL)
                    .stroke(egui::Stroke::new(1.0, theme::DIVIDER))
                    .corner_radius(egui::CornerRadius::same(8))
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.heading(language.pick("USB PD 时间线", "USB PD Timeline"));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(egui::RichText::new(&self.status).small().color(theme::MUTED_TEXT));
                            });
                        });
                        ui.separator();
                        let timeline = pd_timeline_entries(
                            &self.pd_log,
                            &self.pd_trace_log,
                            self.pd_protocol_visible,
                            self.pd_trace_enabled,
                        );
                        egui::ScrollArea::vertical()
                            .auto_shrink([false; 2])
                            .stick_to_bottom(self.pd_auto_scroll)
                            .show(ui, |ui| {
                                if timeline.is_empty() {
                                    ui.vertical_centered(|ui| {
                                        ui.add_space(60.0);
                                        ui.label(
                                            egui::RichText::new(language.pick(
                                                "等待 KM003C 的 PD 报文",
                                                "Waiting for KM003C PD messages",
                                            ))
                                            .size(18.0),
                                        );
                                        ui.label(
                                            egui::RichText::new(language.pick(
                                                "连接设备并触发一次 USB-C 协商后，报文会显示在这里。",
                                                "Connect a device and trigger USB-C negotiation; messages will appear here.",
                                            ))
                                                .color(theme::MUTED_TEXT),
                                        );
                                    });
                                }
                                for timeline_entry in timeline {
                                    match timeline_entry {
                                        PdTimelineEntry::Protocol(entry) => {
                                            let color = match entry.category {
                                                PdCategory::Connect => theme::CURRENT,
                                                PdCategory::Disconnect | PdCategory::Error => theme::RECORDING,
                                                PdCategory::SourceCaps => theme::VOLTAGE,
                                                PdCategory::Request => theme::POWER,
                                                PdCategory::Contract => theme::CURRENT,
                                                PdCategory::Control => theme::MUTED_TEXT,
                                                PdCategory::Extended => egui::Color32::from_rgb(0xC6, 0x7A, 0xD9),
                                            };
                                            egui::Frame::NONE
                                                .fill(color.gamma_multiply(0.06))
                                                .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.25)))
                                                .corner_radius(egui::CornerRadius::same(5))
                                                .inner_margin(egui::Margin::symmetric(9, 6))
                                                .show(ui, |ui| {
                                                    ui.colored_label(
                                                        color,
                                                        egui::RichText::new(format!("[WIRE] {}", entry.summary))
                                                            .monospace()
                                                            .strong(),
                                                    );
                                                    for detail in &entry.details {
                                                        ui.label(
                                                            egui::RichText::new(format!("        {detail}"))
                                                                .monospace()
                                                                .color(color.gamma_multiply(0.82)),
                                                        );
                                                    }
                                                });
                                            ui.add_space(4.0);
                                        }
                                        PdTimelineEntry::FirmwareTrace(entry) => {
                                            let color = match entry.category {
                                                PdTraceCategory::TypeCState => theme::VOLTAGE,
                                                PdTraceCategory::ProtocolEvent => theme::CURRENT,
                                                PdTraceCategory::Unknown => theme::POWER,
                                            };
                                            ui.colored_label(
                                                color,
                                                egui::RichText::new(format!("[FW]   {}", entry.summary)).monospace(),
                                            );
                                        }
                                    }
                                }
                            });
                    });
            });
    }
}

impl PowerMonitorApp {
    fn show_settings_window(&mut self, ctx: &egui::Context) {
        if !self.settings_open {
            return;
        }
        let mut open = self.settings_open;
        let metrics = SettingsLayoutMetrics::for_content_rect(ctx.content_rect());
        let mut close_requested = false;
        egui::Window::new(self.language.pick("工作台设置", "Workbench Settings"))
            .id(egui::Id::new("settings_window_fixed_columns_v2"))
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .fixed_size(metrics.window_size)
            .resizable(false)
            .collapsible(false)
            .open(&mut open)
            .show(ctx, |ui| {
                let available_size = ui.available_size();
                ui.allocate_ui_with_layout(
                    available_size,
                    egui::Layout::left_to_right(egui::Align::Min).with_main_wrap(false),
                    |columns| {
                        columns.spacing_mut().item_spacing.x = metrics.column_gap;
                        columns.allocate_ui_with_layout(
                            egui::vec2(metrics.navigation_width, available_size.y),
                            egui::Layout::top_down(egui::Align::Min),
                            |navigation| {
                                egui::Frame::NONE
                                    .fill(theme::PANEL)
                                    .corner_radius(egui::CornerRadius::same(6))
                                    .inner_margin(egui::Margin::same(8))
                                    .show(navigation, |navigation| {
                                        let navigation_content_width = metrics.navigation_width - 16.0;
                                        navigation.set_width(navigation_content_width);
                                        navigation.set_max_width(navigation_content_width);
                                        navigation.set_min_height((available_size.y - 16.0).max(0.0));
                                        navigation.label(
                                            egui::RichText::new(self.language.pick("工作台", "WORKBENCH"))
                                                .small()
                                                .strong()
                                                .color(theme::TEXT_MUTED),
                                        );
                                        navigation.add_space(8.0);
                                        for page in SettingsPage::ALL {
                                            let selected = self.settings_page == page;
                                            let label = page.localized_label(self.language);
                                            let button = egui::Button::new(egui::RichText::new(label).strong().color(
                                                if selected {
                                                    theme::TEXT_PRIMARY
                                                } else {
                                                    theme::TEXT_SECONDARY
                                                },
                                            ))
                                            .selected(selected)
                                            .fill(if selected {
                                                theme::PANEL_RAISED
                                            } else {
                                                egui::Color32::TRANSPARENT
                                            })
                                            .stroke(egui::Stroke::NONE)
                                            .corner_radius(egui::CornerRadius::same(6));
                                            if navigation
                                                .add_sized([navigation_content_width, 38.0], button)
                                                .on_hover_text(label)
                                                .clicked()
                                            {
                                                self.settings_page = page;
                                            }
                                        }
                                    });
                            },
                        );

                        let content_size = egui::vec2(columns.available_width(), available_size.y);
                        columns.allocate_ui_with_layout(
                            content_size,
                            egui::Layout::top_down(egui::Align::Min),
                            |content| {
                                content.set_width(content_size.x);
                                content.set_max_width(content_size.x);
                                content.add(
                                    egui::Label::new(
                                        egui::RichText::new(self.settings_page.localized_label(self.language))
                                            .strong()
                                            .size(20.0),
                                    )
                                    .wrap(),
                                );
                                content.add(
                                    egui::Label::new(
                                        egui::RichText::new(self.settings_page.localized_description(self.language))
                                            .color(theme::TEXT_SECONDARY),
                                    )
                                    .wrap(),
                                );
                                content.add_space(10.0);

                                let scroll_height =
                                    (content.available_height() - metrics.footer_height - 8.0).max(160.0);
                                content.allocate_ui_with_layout(
                                    egui::vec2(content_size.x, scroll_height),
                                    egui::Layout::top_down(egui::Align::Min),
                                    |scroll_host| {
                                        scroll_host.set_width(content_size.x);
                                        scroll_host.set_max_width(content_size.x);
                                        let scroll_content_width = scroll_host.available_width();
                                        egui::ScrollArea::vertical()
                                            .id_salt(("settings_content_scroll_v2", self.settings_page as u8))
                                            .max_height(scroll_height)
                                            .min_scrolled_height(scroll_height)
                                            .auto_shrink([false; 2])
                                            .show(scroll_host, |settings| {
                                                settings.set_width(scroll_content_width);
                                                settings.set_max_width(scroll_content_width);
                                                self.show_settings_content(settings);
                                            });
                                    },
                                );

                                content.separator();
                                content.allocate_ui_with_layout(
                                    egui::vec2(content_size.x, metrics.footer_height),
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |footer| {
                                        if footer
                                            .add_sized(
                                                [78.0, 32.0],
                                                egui::Button::new(self.language.pick("完成", "Done")),
                                            )
                                            .clicked()
                                        {
                                            close_requested = true;
                                        }
                                        footer.add(
                                            egui::Label::new(
                                                egui::RichText::new(self.language.pick(
                                                    "更改会立即应用并自动保存",
                                                    "Changes apply immediately and are saved automatically",
                                                ))
                                                .small()
                                                .color(theme::TEXT_MUTED),
                                            )
                                            .truncate(),
                                        );
                                    },
                                );
                            },
                        );
                    },
                );
            });
        self.settings_open = open && !close_requested;
    }

    fn show_settings_content(&mut self, ui: &mut egui::Ui) {
        let language = self.language;
        if self.settings_page == SettingsPage::General {
            settings_section(ui, language.pick("界面", "Interface"), true, |ui| {
                let control_width = settings_control_width(ui.available_width());
                egui::Grid::new("settings_interface_grid")
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        settings_form_label(ui, language.pick("界面语言", "Language"));
                        egui::ComboBox::from_id_salt("settings_language")
                            .width(control_width)
                            .selected_text(self.language.native_name())
                            .show_ui(ui, |ui| {
                                for option in Language::ALL {
                                    ui.selectable_value(&mut self.language, option, option.native_name());
                                }
                            });
                        ui.end_row();
                    });
            });

            settings_section(ui, language.pick("数据源", "Data source"), true, |ui| {
                let previous_source = self.plot_source;
                ui.horizontal_wrapped(|ui| {
                    ui.selectable_value(&mut self.plot_source, PlotSource::Live, language.pick("实时", "Live"));
                    ui.add_enabled_ui(self.offline_view.is_some(), |ui| {
                        ui.selectable_value(
                            &mut self.plot_source,
                            PlotSource::Offline,
                            language.pick("设备离线", "On-device"),
                        );
                    });
                    ui.add_enabled_ui(self.imported_recording.is_some(), |ui| {
                        ui.selectable_value(
                            &mut self.plot_source,
                            PlotSource::Imported,
                            language.pick("桌面导入", "Imported file"),
                        );
                    });
                });
                if previous_source != self.plot_source {
                    self.cursor_readout = None;
                    self.cursor_pinned = false;
                    self.chart_viewport.selection = None;
                    self.reset_plots_requested = true;
                    if self.plot_source != PlotSource::Live {
                        self.time_window = TimeWindow::All;
                    }
                    self.chart_follow_mode = self.preferred_follow_mode();
                }
                ui.label(
                    egui::RichText::new(language.pick(
                        "导入和关闭文件请使用监控工具栏或导入文件顶部的信息条。",
                        "Import and close files from the Monitor toolbar or the imported-file banner.",
                    ))
                    .small()
                    .color(theme::MUTED_TEXT),
                );
            });

            settings_section(ui, language.pick("设备信息", "Device information"), false, |ui| {
                if let Some(state) = &self.device_state {
                    let value_width = settings_control_width(ui.available_width());
                    egui::Grid::new("settings_device_info")
                        .num_columns(2)
                        .spacing([18.0, 4.0])
                        .show(ui, |ui| {
                            for (label, value) in [
                                (language.pick("型号", "Model"), state.info.model.as_str()),
                                (language.pick("固件", "Firmware"), state.info.fw_version.as_str()),
                                (language.pick("硬件", "Hardware"), state.info.hw_version.as_str()),
                                (language.pick("序列号", "Serial number"), state.info.serial_id.as_str()),
                            ] {
                                settings_form_label(ui, label);
                                ui.add_sized(
                                    [value_width, ui.spacing().interact_size.y],
                                    egui::Label::new(egui::RichText::new(value).monospace()).truncate(),
                                )
                                .on_hover_text(value);
                                ui.end_row();
                            }
                        });
                } else {
                    ui.label(
                        egui::RichText::new(language.pick("未连接 KM003C", "KM003C is not connected"))
                            .color(theme::MUTED_TEXT),
                    );
                }
                ui.checkbox(
                &mut self.usb_reset,
                language.pick("连接时执行 USB reset（高级）", "Run USB reset when connecting (Advanced)"),
            )
            .on_hover_text(language.pick(
                "macOS 默认跳过 USB reset；只在设备异常且你明确需要时开启。",
                "USB reset is skipped by default on macOS. Enable it only when the device is malfunctioning and a reset is required.",
            ));
            });
        }

        if self.settings_page == SettingsPage::Recording {
            settings_section(ui, language.pick("录制", "Recording"), true, |ui| {
                let mut auto_rule_changed = false;
                let control_width = settings_control_width(ui.available_width());
                egui::Grid::new("settings_recording_grid")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    settings_form_label(ui, language.pick("文件格式", "File format"));
                    ui.add_enabled_ui(self.recorder.is_none(), |ui| {
                        egui::ComboBox::from_id_salt("settings_recording_format")
                            .width(control_width)
                            .selected_text(self.recording_format.label())
                            .show_ui(ui, |ui| {
                                for format in RecordingFormat::ALL {
                                    ui.selectable_value(&mut self.recording_format, format, format.label());
                                }
                            });
                    });
                    ui.end_row();

                    settings_form_label(ui, language.pick("锁屏保护", "Lock-screen protection"));
                    ui.vertical(|ui| {
                        ui.add_enabled_ui(!self.recording_session, |ui| {
                            ui.checkbox(
                                &mut self.sleep_protection_enabled,
                                language.pick(
                                    "录制时阻止 Mac 空闲睡眠",
                                    "Prevent idle system sleep while recording",
                                ),
                            )
                            .on_hover_text(language.pick(
                                "允许锁屏和屏幕熄灭；不会阻止合盖或用户主动睡眠。",
                                "The screen may lock or turn off. Closing the lid or explicitly choosing Sleep can still suspend USB.",
                            ));
                        });
                        if self.recording_session {
                            let protected = self
                                .sleep_assertion
                                .as_ref()
                                .is_some_and(IdleSleepAssertion::is_active);
                            ui.colored_label(
                                if protected { theme::CURRENT } else { theme::POWER },
                                egui::RichText::new(if protected {
                                    language.pick("● 防空闲睡眠已生效", "● Idle-sleep protection active")
                                } else {
                                    language.pick("○ 当前未建立睡眠保护", "○ Sleep protection is not active")
                                })
                                .small(),
                            );
                        }
                    });
                    ui.end_row();

                    settings_form_label(ui, language.pick("自动暂停/继续", "Auto pause/resume"));
                    auto_rule_changed |= ui
                        .checkbox(
                            &mut self.auto_pause_enabled,
                            language.pick("启用同一段自动控制", "Enable within-session automation"),
                        )
                        .changed();
                    ui.end_row();

                    settings_form_label(ui, language.pick("判断指标", "Metric"));
                    ui.add_enabled_ui(self.auto_pause_enabled, |ui| {
                        let previous = self.auto_capture_metric;
                        egui::ComboBox::from_id_salt("settings_auto_capture_metric")
                            .width(control_width)
                            .selected_text(self.auto_capture_metric.localized_label(language))
                            .show_ui(ui, |ui| {
                                for metric in AutoCaptureMetric::ALL {
                                    ui.selectable_value(
                                        &mut self.auto_capture_metric,
                                        metric,
                                        metric.localized_label(language),
                                    );
                                }
                            });
                        auto_rule_changed |= previous != self.auto_capture_metric;
                    });
                    ui.end_row();

                    settings_form_label(ui, language.pick("触发阈值", "Trigger threshold"));
                    ui.add_enabled_ui(self.auto_pause_enabled, |ui| {
                        let maximum = match self.auto_capture_metric {
                            AutoCaptureMetric::Power => 100_000,
                            AutoCaptureMetric::Current => 20_000,
                            AutoCaptureMetric::Voltage => 50_000,
                        };
                        auto_rule_changed |= ui
                            .add(
                                egui::DragValue::new(&mut self.auto_pause_threshold_mw)
                                    .range(0..=maximum)
                                    .suffix(format!(" {}", self.auto_capture_metric.milli_unit())),
                            )
                            .changed();
                    });
                    ui.end_row();

                    settings_form_label(ui, language.pick("持续时间", "Hold time"));
                    ui.add_enabled_ui(self.auto_pause_enabled, |ui| {
                        let mut seconds = self.auto_pause_delay_ms as f64 / 1_000.0;
                        if ui
                            .add(egui::DragValue::new(&mut seconds).range(0.1..=600.0).suffix(" s"))
                            .changed()
                        {
                            self.auto_pause_delay_ms = (seconds * 1_000.0).round() as u32;
                            auto_rule_changed = true;
                        }
                    });
                    ui.end_row();
                });
                if self.recorder.is_some() || self.recording_session {
                    ui.label(
                        egui::RichText::new(language.pick(
                            "录制进行中：文件格式和睡眠保护将在本段结束后才能修改。",
                            "Recording is active. File format and sleep protection can be changed after this session ends.",
                        ))
                        .small()
                        .color(theme::POWER),
                    );
                }
                if auto_rule_changed {
                    self.auto_pause_below_since_us = None;
                    self.auto_resume_above_since_us = None;
                }
                ui.label(
                egui::RichText::new(language.pick(
                    "低于阈值达到持续时间后自动暂停；只有自动暂停才会在超过回差阈值后自动继续。手动暂停必须手动继续。",
                    "The session pauses after the value stays below the threshold. Only an automatic pause can resume after the hysteresis threshold is exceeded; a manual pause always requires manual resume.",
                ))
                    .small()
                    .color(theme::MUTED_TEXT),
            );
            });

            let pending_directory = application_recordings_directory().join("Pending");
            let recoverable_sessions = discover_recoverable_sessions(&pending_directory);
            let recoverable = recoverable_recordings();
            if !recoverable_sessions.is_empty() || !recoverable.is_empty() {
                let title = format!(
                    "{} ({})",
                    language.pick("可恢复录制", "Recoverable recordings"),
                    recoverable_sessions.len() + recoverable.len()
                );
                settings_section(ui, &title, false, |ui| {
                    let mut import_path = None;
                    let mut view_session = None;
                    let mut save_session = None;
                    let mut continue_session = None;
                    egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                        let session_card_width = ui.available_width();
                        let session_inner_width = (session_card_width - 14.0).max(404.0);
                        for (directory, manifest) in &recoverable_sessions {
                            egui::Frame::NONE
                                .fill(theme::PANEL)
                                .corner_radius(egui::CornerRadius::same(6))
                                .inner_margin(egui::Margin::symmetric(7, 5))
                                .show(ui, |ui| {
                                    ui.set_width(session_inner_width);
                                    ui.set_max_width(session_inner_width);
                                    let (detail_width, action_width) = recoverable_session_columns(session_inner_width);
                                    ui.allocate_ui_with_layout(
                                        egui::vec2(session_inner_width, 44.0),
                                        egui::Layout::left_to_right(egui::Align::Center),
                                        |row| {
                                            row.spacing_mut().item_spacing.x = 12.0;
                                            row.allocate_ui_with_layout(
                                                egui::vec2(detail_width, 44.0),
                                                egui::Layout::top_down(egui::Align::Min),
                                                |details| {
                                                    details.set_width(detail_width);
                                                    details
                                                        .add_sized(
                                                            [detail_width, 20.0],
                                                            egui::Label::new(
                                                                egui::RichText::new(
                                                                    &manifest.metadata.timestamps.started_at_beijing,
                                                                )
                                                                .monospace()
                                                                .small(),
                                                            )
                                                            .truncate(),
                                                        )
                                                        .on_hover_text(
                                                            &manifest.metadata.timestamps.started_at_beijing,
                                                        );
                                                    details.add_sized(
                                                        [detail_width, 18.0],
                                                        egui::Label::new(
                                                            egui::RichText::new(format!(
                                                                "{} SPS · {} pts · {}",
                                                                manifest.metadata.sample_rate_hz,
                                                                manifest.metadata.rows,
                                                                localized_session_state(manifest.state, language,),
                                                            ))
                                                            .small()
                                                            .color(theme::MUTED_TEXT),
                                                        )
                                                        .truncate(),
                                                    );
                                                },
                                            );
                                            row.allocate_ui_with_layout(
                                                egui::vec2(action_width, 44.0),
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |actions| {
                                                    actions.spacing_mut().item_spacing.x =
                                                        RECOVERABLE_SESSION_ACTION_GAP;
                                                    if actions
                                                        .add(egui::Button::new(language.pick("保存", "Save")).min_size(
                                                            egui::vec2(RECOVERABLE_SESSION_SECONDARY_WIDTH, 28.0),
                                                        ))
                                                        .clicked()
                                                    {
                                                        save_session = Some((directory.clone(), manifest.clone()));
                                                    }
                                                    if actions
                                                        .add(egui::Button::new(language.pick("查看", "View")).min_size(
                                                            egui::vec2(RECOVERABLE_SESSION_SECONDARY_WIDTH, 28.0),
                                                        ))
                                                        .clicked()
                                                    {
                                                        view_session = Some((directory.clone(), manifest.clone()));
                                                    }
                                                    if actions
                                                        .add_enabled(
                                                            !self.recording_session && self.device_state.is_some(),
                                                            egui::Button::new(language.pick("继续录制", "Continue"))
                                                                .min_size(egui::vec2(
                                                                    RECOVERABLE_SESSION_CONTINUE_WIDTH,
                                                                    28.0,
                                                                )),
                                                        )
                                                        .clicked()
                                                    {
                                                        continue_session = Some((directory.clone(), manifest.clone()));
                                                    }
                                                },
                                            );
                                        },
                                    );
                                });
                            ui.add_space(4.0);
                        }
                        if !recoverable.is_empty() {
                            let (filename_width, button_width) = recoverable_file_columns(ui.available_width());
                            egui::Grid::new("recoverable_legacy_files_grid")
                                .num_columns(2)
                                .spacing([RECOVERABLE_FILE_COLUMN_GAP, 6.0])
                                .show(ui, |ui| {
                                    for path in &recoverable {
                                        let name = path
                                            .file_name()
                                            .and_then(|name| name.to_str())
                                            .unwrap_or(language.pick("录制文件", "Recording file"));
                                        ui.add_sized(
                                            [filename_width, 30.0],
                                            egui::Label::new(egui::RichText::new(name).monospace().small()).truncate(),
                                        )
                                        .on_hover_text(path.display().to_string());
                                        if ui
                                            .add_sized(
                                                [button_width, 28.0],
                                                egui::Button::new(language.pick("导入", "Import")),
                                            )
                                            .clicked()
                                        {
                                            import_path = Some(path.clone());
                                        }
                                        ui.end_row();
                                    }
                                });
                        }
                    });
                    if let Some(path) = import_path {
                        self.start_recording_import(path);
                    }
                    if let Some((directory, manifest)) = view_session {
                        self.open_recoverable_session(directory, manifest);
                    }
                    if let Some((directory, manifest)) = save_session {
                        self.save_recoverable_session(directory, manifest);
                    }
                    if let Some((directory, manifest)) = continue_session {
                        self.continue_recoverable_session(directory, manifest);
                    }
                });
            }
        }

        if self.settings_page == SettingsPage::Chart {
            settings_section(ui, language.pick("图表", "Chart"), true, |ui| {
                let previous_window = self.time_window;
                let control_width = settings_control_width(ui.available_width());
                egui::Grid::new("settings_chart_grid")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    settings_form_label(ui, language.pick("默认时间窗", "Default time window"));
                    egui::ComboBox::from_id_salt("settings_time_window")
                        .width(control_width)
                        .selected_text(self.time_window.localized_label(language))
                        .show_ui(ui, |ui| {
                            for window in TimeWindow::all() {
                                ui.selectable_value(
                                    &mut self.time_window,
                                    *window,
                                    window.localized_label(language),
                                );
                            }
                        });
                    ui.end_row();
                    settings_form_label(ui, language.pick("曲线降噪", "Trace smoothing"));
                    let filter_response = egui::ComboBox::from_id_salt("settings_display_filter")
                        .width(control_width)
                        .selected_text(self.display_filter.localized_label(language))
                        .show_ui(ui, |ui| {
                            for filter in [DisplayFilter::Median5, DisplayFilter::Raw] {
                                ui.selectable_value(
                                    &mut self.display_filter,
                                    filter,
                                    filter.localized_label(language),
                                );
                            }
                        });
                    filter_response
                        .response
                        .on_hover_text(language.pick(
                            "五点中值滤波只改变屏幕曲线；游标、统计、录制和导出始终使用原始采样。",
                            "The 5-point median filter affects only the displayed traces. Cursor values, statistics, recordings, and exports always use raw samples.",
                        ));
                    ui.end_row();
                });
                if self.time_window != previous_window {
                    self.chart_follow_mode = if self.time_window == TimeWindow::All {
                        ChartFollowMode::FullSession
                    } else {
                        ChartFollowMode::LatestWindow
                    };
                    self.chart_viewport.selection = None;
                }
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new(language.pick("高级曲线", "Advanced traces")).color(theme::MUTED_TEXT),
                    );
                    for (index, metric) in self.plot_metrics.iter_mut().enumerate() {
                        egui::ComboBox::from_id_salt(("advanced_metric", index))
                            .width(82.0)
                            .selected_text(metric.localized_label(language))
                            .show_ui(ui, |ui| {
                                for option in PlotMetric::ALL {
                                    if self.plot_source != PlotSource::Offline || option.supports_offline() {
                                        ui.selectable_value(metric, option, option.localized_label(language));
                                    }
                                }
                            });
                    }
                });
                if ui
                    .button(language.pick("打开高级分析窗口", "Open Advanced Analysis"))
                    .clicked()
                {
                    self.advanced_analysis_open = true;
                }
            });
        }

        if self.settings_page == SettingsPage::DataAndDevice {
            settings_section(ui, language.pick("数据质量", "Data quality"), false, |ui| {
                let value_width = settings_control_width(ui.available_width());
                egui::Grid::new("settings_data_quality")
                    .num_columns(2)
                    .spacing([12.0, 4.0])
                    .show(ui, |ui| {
                        for (label, value) in [
                            (
                                language.pick("接收采样", "Received samples"),
                                self.total_samples.to_string(),
                            ),
                            (
                                language.pick("缺失采样", "Missing samples"),
                                self.dropped_samples.to_string(),
                            ),
                            (
                                language.pick("乱序/重复丢弃", "Out-of-order / duplicate samples discarded"),
                                self.discarded_sequence_samples.to_string(),
                            ),
                            (
                                language.pick("缓冲区", "Buffer"),
                                format!("{} {}", self.data_points.len(), language.pick("点", "points")),
                            ),
                        ] {
                            settings_form_label(ui, label);
                            ui.add_sized(
                                [value_width, ui.spacing().interact_size.y],
                                egui::Label::new(egui::RichText::new(value).monospace()).truncate(),
                            );
                            ui.end_row();
                        }
                    });
            });

            settings_section(
                ui,
                language.pick("设备离线记录", "On-device recordings"),
                false,
                |ui| {
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(
                                self.device_state.is_some()
                                    && !self.offline_busy
                                    && self.recorder.is_none()
                                    && self.offline_export.is_none(),
                                egui::Button::new(language.pick("刷新目录", "Refresh list")),
                            )
                            .clicked()
                        {
                            self.request_offline_catalog();
                        }
                        if self.offline_busy {
                            ui.spinner();
                        }
                    });
                    if !self.offline_catalog.is_empty() {
                        let control_width = settings_control_width(ui.available_width());
                        let selected_text = self
                            .offline_selected
                            .and_then(|index| self.offline_catalog.get(index))
                            .map_or_else(
                                || language.pick("选择记录", "Select a recording").to_string(),
                                |metadata| metadata.filename_lossy().into_owned(),
                            );
                        egui::ComboBox::from_id_salt("settings_offline_recording")
                            .width(control_width)
                            .truncate()
                            .selected_text(selected_text)
                            .show_ui(ui, |ui| {
                                for (index, metadata) in self.offline_catalog.iter().enumerate() {
                                    ui.selectable_value(
                                        &mut self.offline_selected,
                                        Some(index),
                                        format!(
                                            "{} · {} {}",
                                            metadata.filename_lossy(),
                                            metadata.sample_count,
                                            language.pick("点", "points")
                                        ),
                                    );
                                }
                            });
                        if ui
                            .add_enabled(
                                self.offline_selected.is_some() && !self.offline_busy && self.recorder.is_none(),
                                egui::Button::new(language.pick("下载并查看", "Download and view")),
                            )
                            .clicked()
                        {
                            self.download_selected_offline_log();
                        }
                    }
                    if self.offline_view.is_some()
                        && ui
                            .add_enabled(
                                self.offline_export.is_none() && self.recorder.is_none(),
                                egui::Button::new(language.pick("导出已下载记录", "Export downloaded recording")),
                            )
                            .clicked()
                    {
                        self.export_offline_log();
                    }
                    let offline_status = if self.offline_status == "尚未加载设备离线记录" {
                        language.pick("尚未加载设备离线记录", "No on-device recordings have been loaded")
                    } else {
                        &self.offline_status
                    };
                    ui.add(
                        egui::Label::new(egui::RichText::new(offline_status).small().color(theme::MUTED_TEXT)).wrap(),
                    );
                },
            );
        }

        if self.settings_page == SettingsPage::Diagnostics {
            settings_section(ui, language.pick("应用信息", "Application"), true, |ui| {
                let value_width = settings_control_width(ui.available_width());
                egui::Grid::new("settings_application_info")
                    .num_columns(2)
                    .spacing([18.0, 6.0])
                    .show(ui, |ui| {
                        for (label, value) in [
                            (
                                language.pick("版本", "Version"),
                                format!("{} v{APP_VERSION} ({APP_BUILD})", i18n::app_title(language)),
                            ),
                            (
                                language.pick("底层核心", "Core"),
                                "km003c-rs · MIT / Apache-2.0".to_string(),
                            ),
                            (
                                language.pick("诊断日志", "Diagnostic logs"),
                                "~/Library/Application Support/com.weixun.km003cworkbench/logs/".to_string(),
                            ),
                        ] {
                            settings_form_label(ui, label);
                            ui.add_sized(
                                [value_width, ui.spacing().interact_size.y],
                                egui::Label::new(egui::RichText::new(&value).monospace().color(theme::TEXT_PRIMARY))
                                    .truncate(),
                            )
                            .on_hover_text(value);
                            ui.end_row();
                        }
                    });
            });
            settings_section(ui, language.pick("开源与声明", "Sources & notices"), false, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.hyperlink_to("okhsunrog/km003c-rs", "https://github.com/okhsunrog/km003c-rs");
                    ui.separator();
                    ui.hyperlink_to("KHWLGH/WITRN-RS", "https://github.com/KHWLGH/WITRN-RS");
                });
                ui.label(
                    egui::RichText::new(language.pick(
                        "基于 km003c-rs；维简式截图仅用于交互研究。本软件独立实现，不复制第三方代码、资源或品牌，也不是 ChargerLAB 官方软件。",
                        "Built on km003c-rs. The WITRN-style screenshot was used only for interaction research. This independent implementation copies no third-party code, assets, or branding and is not official ChargerLAB software.",
                    ))
                    .small()
                    .color(theme::TEXT_SECONDARY),
                );
            });
        }
    }

    fn show_advanced_analysis_window(&mut self, ctx: &egui::Context) {
        if !self.advanced_analysis_open {
            return;
        }
        let mut open = self.advanced_analysis_open;
        let language = self.language;
        egui::Window::new(language.pick("高级曲线分析", "Advanced Trace Analysis"))
            .id(egui::Id::new("advanced_analysis_window"))
            .default_size([900.0, 620.0])
            .min_size([680.0, 460.0])
            .open(&mut open)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(self.source_label());
                    ui.label(
                        egui::RichText::new(language.pick(
                            "主监控页始终保留 V / A / W",
                            "The main monitor keeps V / A / W available",
                        ))
                        .small()
                        .color(theme::MUTED_TEXT),
                    );
                });
                let selection = self.ensure_chart_selection();
                let plot_height = ((ui.available_height() - 28.0) / 3.0).max(110.0);
                let maximum_points = (ui.available_width() * 2.0) as usize;
                for (index, metric) in self.plot_metrics.into_iter().enumerate() {
                    let points = self.source_metric_points(metric, selection, maximum_points);
                    Plot::new(("advanced_analysis_plot", index))
                        .height(plot_height)
                        .show_grid([true, true])
                        .link_axis("advanced_analysis_axis", [true, false])
                        .link_cursor("advanced_analysis_cursor", [true, false])
                        .legend(Legend::default().position(Corner::RightTop).background_alpha(0.65))
                        .x_axis_formatter(|mark, _| format_plot_time(mark.value))
                        .show(ui, |plot_ui| {
                            plot_ui.line(
                                Line::new(
                                    format!("{} ({})", metric.localized_label(language), metric.unit()),
                                    PlotPoints::from(points),
                                )
                                .color(metric.color())
                                .width(1.5),
                            );
                        });
                }
            });
        self.advanced_analysis_open = open;
    }
}

fn recoverable_recordings() -> Vec<PathBuf> {
    let directory = application_recordings_directory().join("Pending");
    let mut paths = std::fs::read_dir(directory)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && matches!(
                    path.extension().and_then(|value| value.to_str()),
                    Some("csv" | "parquet")
                )
        })
        .collect::<Vec<_>>();
    paths.sort_by(|left, right| right.file_name().cmp(&left.file_name()));
    paths
}

impl eframe::App for PowerMonitorApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.show_workbench(ui);
        #[cfg(any())]
        {
            self.process_messages();
            self.update_demo_data();

            // Request repaints - fast when streaming, slower when idle
            if self.streaming && self.plot_source == PlotSource::Live {
                ui.ctx().request_repaint_after(Duration::from_millis(16)); // ~60fps when streaming
            } else {
                ui.ctx().request_repaint_after(Duration::from_millis(100)); // 10fps when idle
            }

            // Top status rail: phase is written as text and color, so color is not
            // the only way to understand a connection or recording condition.
            egui::Panel::top("header").show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.heading(APP_TITLE);
                    ui.separator();
                    let status_color = match self.phase {
                        ConnectionPhase::Streaming => theme::VOLTAGE,
                        ConnectionPhase::Searching | ConnectionPhase::Connecting => theme::POWER,
                        ConnectionPhase::NoDevice | ConnectionPhase::Disconnected => {
                            egui::Color32::from_rgb(0xB8, 0xC3, 0xCF)
                        }
                        ConnectionPhase::DeviceBusy | ConnectionPhase::ConnectionError => {
                            egui::Color32::from_rgb(0xFF, 0x76, 0x76)
                        }
                    };
                    ui.colored_label(status_color, i18n::connection_status(self.language, self.phase));
                    ui.colored_label(status_color, &self.status);
                    ui.separator();
                    ui.monospace(format!(
                        "{} · v{} ({})",
                        self.current_rate.label(),
                        APP_VERSION,
                        APP_BUILD
                    ));
                    if self.demo_mode {
                        ui.colored_label(theme::POWER, "演示数据");
                    }
                    if self.recording_session {
                        let recording_label = if self.recorder.as_ref().is_some_and(|recorder| recorder.is_finishing())
                        {
                            "正在保存"
                        } else if self.recording_paused {
                            "录制已暂停"
                        } else {
                            "录制中"
                        };
                        ui.colored_label(theme::POWER, recording_label);
                    } else if self.recorder.is_some() {
                        ui.colored_label(theme::POWER, "正在导出");
                    }
                });
            });

            // Large, glanceable values stay above the plots at all window sizes.
            egui::Frame::group(ui.style())
                .fill(theme::BACKPLANE)
                .stroke(egui::Stroke::new(1.0, theme::DIVIDER))
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.horizontal_wrapped(|ui| {
                        metric_card(ui, "电压", self.current_voltage, "V", theme::VOLTAGE);
                        metric_card(ui, "电流", self.current_current.abs(), "A", theme::CURRENT);
                        metric_card(ui, "功率", self.current_power.abs(), "W", theme::POWER);
                    });
                });

            // Prominent recording controls follow the instrument-style toolbar
            // used by dedicated USB power meters. The live graph remains the
            // existing KM003C view, while this strip makes the saved segment and
            // its two key totals visible at a glance.
            self.show_recording_toolbar(ui);

            // Left panel with device info and controls
            egui::Panel::left("info_panel").min_size(280.0).show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
            ui.heading("设备信息");
            ui.separator();

            if let Some(state) = &self.device_state {
                egui::Grid::new("device_info_grid")
                    .num_columns(2)
                    .spacing([10.0, 4.0])
                    .show(ui, |ui| {
                        ui.label("型号");
                        ui.label(&state.info.model);
                        ui.end_row();

                        ui.label("固件");
                        ui.label(&state.info.fw_version);
                        ui.end_row();

                        ui.label("固件日期");
                        ui.label(&state.info.fw_date);
                        ui.end_row();

                        ui.label("硬件版本");
                        ui.label(&state.info.hw_version);
                        ui.end_row();

                        ui.label("生产日期");
                        ui.label(&state.info.mfg_date);
                        ui.end_row();

                        ui.label("序列号");
                        ui.label(&state.info.serial_id);
                        ui.end_row();

                        ui.label("硬件 ID");
                        ui.label(format!("{}", state.hardware_id));
                        ui.end_row();

                        ui.label("鉴权级别");
                        ui.label(format!("{}", state.auth_level));
                        ui.end_row();

                        ui.label("AdcQueue");
                        ui.colored_label(
                            if state.adcqueue_enabled {
                                egui::Color32::GREEN
                            } else {
                                egui::Color32::RED
                            },
                            if state.adcqueue_enabled { "已启用" } else { "未启用" },
                        );
                        ui.end_row();
                    });
            } else {
                ui.label("未连接");
            }

            ui.add_space(20.0);
            ui.separator();
            ui.heading("实时读数");
            ui.separator();

            instrument_readout_card(
                ui,
                "电压",
                self.current_voltage,
                "V",
                theme::VOLTAGE,
                self.recording_statistics.voltage.readout(),
            );
            ui.add_space(5.0);
            instrument_readout_card(
                ui,
                "电流",
                self.current_current.abs(),
                "A",
                theme::CURRENT,
                self.recording_statistics.current.readout(),
            );
            ui.add_space(5.0);
            instrument_readout_card(
                ui,
                "功率",
                self.current_power.abs(),
                "W",
                theme::POWER,
                self.recording_statistics.power.readout(),
            );

            ui.add_space(10.0);
            if let Some(accumulated) = self.accumulated_readout() {
                ui.label(egui::RichText::new("累计参数").strong());
                egui::Grid::new("accumulated_grid")
                    .num_columns(2)
                    .spacing([10.0, 4.0])
                    .show(ui, |ui| {
                        ui.colored_label(theme::POWER, "累计能量");
                        ui.monospace(format_cumulative_energy(accumulated.cumulative_energy_uwh));
                        ui.end_row();
                        ui.colored_label(theme::CURRENT, "累计容量");
                        ui.monospace(format_capacity(accumulated.capacity_uah));
                        ui.end_row();
                        ui.colored_label(theme::POWER, "净能量");
                        ui.monospace(
                            EnergyPresentation::for_values([accumulated.net_energy_uwh])
                                .format_directional(accumulated.net_energy_uwh),
                        );
                        ui.end_row();
                    });
            }

            ui.add_space(20.0);
            ui.separator();
            ui.heading("PD 状态");
            ui.separator();

            if let Some(pd) = &self.pd_status {
                egui::Grid::new("pd_status_grid")
                    .num_columns(2)
                    .spacing([10.0, 4.0])
                    .show(ui, |ui| {
                        ui.label("CC1");
                        let cc1_v = pd.cc1.get::<volt>();
                        let cc1_color = if self.pd_connection.connected() == Some(true) && cc1_v > 0.2 {
                            egui::Color32::GREEN
                        } else {
                            egui::Color32::GRAY
                        };
                        ui.colored_label(cc1_color, format!("{cc1_v:.3} V"));
                        ui.end_row();

                        ui.label("CC2");
                        let cc2_v = pd.cc2.get::<volt>();
                        let cc2_color = if self.pd_connection.connected() == Some(true) && cc2_v > 0.2 {
                            egui::Color32::GREEN
                        } else {
                            egui::Color32::GRAY
                        };
                        ui.colored_label(cc2_color, format!("{cc2_v:.3} V"));
                        ui.end_row();

                        ui.label("Type-C sink");
                        let (color, label) = match self.pd_connection.connected() {
                            Some(true) => (theme::VOLTAGE, "已连接"),
                            Some(false) => (egui::Color32::from_rgb(0xFF, 0x76, 0x76), "未连接"),
                            None => (theme::POWER, "检测中…"),
                        };
                        ui.colored_label(color, label);
                        ui.end_row();
                    });
            } else {
                ui.label("暂无 PD 数据");
            }

            ui.add_space(20.0);
            ui.separator();
            ui.heading("PD 时间线");
            ui.separator();

            ui.checkbox(&mut self.pd_panel_visible, "显示 PD 面板");
            ui.label("筛选");
            ui.checkbox(&mut self.pd_protocol_visible, "协议报文");
            let trace_changed = ui
                .checkbox(&mut self.pd_trace_enabled, "固件 trace")
                .on_hover_text(
                    "Also drains the diagnostic Type-C and protocol-engine queues reverse engineered from KM003C firmware V1.9.9",
                )
                .changed();
            if trace_changed && self.device_state.is_some() {
                let _ = self
                    .cmd_sender
                    .send(UsbCommand::SetPdTraceEnabled(self.pd_trace_enabled));
            }

            ui.horizontal(|ui| {
                ui.checkbox(&mut self.pd_auto_scroll, "自动滚动");
                if ui.button("清空时间线").clicked() {
                    self.clear_pd_log();
                }
            });
            ui.label(format!(
                "协议：{}  |  Trace：{}",
                self.pd_log.len(),
                self.pd_trace_log.len()
            ));

            ui.add_space(20.0);
            ui.separator();
            ui.heading("采集统计");
            ui.separator();

            egui::Grid::new("stats_grid")
                .num_columns(2)
                .spacing([10.0, 4.0])
                .show(ui, |ui| {
                    ui.label("采样点");
                    ui.label(format!("{}", self.total_samples));
                    ui.end_row();

                    ui.label("丢失");
                    ui.colored_label(
                        if self.dropped_samples > 0 {
                            egui::Color32::RED
                        } else {
                            egui::Color32::GREEN
                        },
                        format!("{}", self.dropped_samples),
                    );
                    ui.end_row();

                    ui.label("丢弃");
                    ui.colored_label(
                        if self.discarded_sequence_samples > 0 {
                            egui::Color32::YELLOW
                        } else {
                            egui::Color32::GREEN
                        },
                        format!("{}", self.discarded_sequence_samples),
                    );
                    ui.end_row();

                    ui.label("缓冲区");
                    ui.label(format!("{} 点", self.data_points.len()));
                    ui.end_row();
                });

            ui.add_space(20.0);
            ui.separator();
            ui.heading("设备与采集控制");
            ui.separator();

            // Sample rate selector
            ui.add_enabled_ui(self.recorder.is_none(), |ui| {
                ui.horizontal(|ui| {
                    ui.label("采样率");
                    let prev_rate = self.selected_rate;
                    egui::ComboBox::from_id_salt("sample_rate")
                        .selected_text(self.selected_rate.label())
                        .show_ui(ui, |ui| {
                            for rate in SampleRateOption::all() {
                                ui.selectable_value(&mut self.selected_rate, *rate, rate.label());
                            }
                        });

                    if self.selected_rate != prev_rate && self.device_state.is_some() {
                        info!("Sample rate changed to {}", self.selected_rate.label());
                        let _ = self
                            .cmd_sender
                            .send(UsbCommand::SetSampleRate(self.selected_rate.to_graph_rate()));
                    }
                });
            });

            ui.collapsing("自动暂停（高级）", |ui| {
                let mut changed = ui
                    .checkbox(&mut self.auto_pause_enabled, "低功率持续后自动暂停")
                    .changed();
                ui.horizontal(|ui| {
                    ui.label("功率阈值");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut self.auto_pause_threshold_mw)
                                .range(0..=100_000)
                                .suffix(" mW"),
                        )
                        .changed();
                });
                let mut delay_seconds = self.auto_pause_delay_ms as f64 / 1_000.0;
                ui.horizontal(|ui| {
                    ui.label("持续时间");
                    if ui
                        .add(
                            egui::DragValue::new(&mut delay_seconds)
                                .range(0.1..=600.0)
                                .speed(0.1)
                                .suffix(" s"),
                        )
                        .changed()
                    {
                        self.auto_pause_delay_ms = (delay_seconds * 1_000.0).round() as u32;
                        changed = true;
                    }
                });
                if changed {
                    self.auto_pause_below_since_us = None;
                }
                ui.small("只暂停录制，实时曲线继续更新；点击“继续记录”可恢复。默认关闭。");
            });

            ui.add_space(5.0);

            // Time window selector
            ui.horizontal(|ui| {
                ui.label("时间窗");
                let previous_window = self.time_window;
                egui::ComboBox::from_id_salt("time_window")
                    .selected_text(self.time_window.label())
                    .show_ui(ui, |ui| {
                        for window in TimeWindow::all() {
                            ui.selectable_value(&mut self.time_window, *window, window.label());
                        }
                    });
                if self.time_window != previous_window {
                    self.cursor_readout = None;
                    self.reset_plots_requested = true;
                }
            });

            ui.add_space(10.0);
            ui.label("曲线指标");
            for (index, metric) in self.plot_metrics.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(format!("{}:", index + 1));
                    egui::ComboBox::from_id_salt(("plot_metric", index))
                        .selected_text(metric.label())
                        .show_ui(ui, |ui| {
                            for option in PlotMetric::ALL {
                                if self.plot_source == PlotSource::Live || option.supports_offline() {
                                    ui.selectable_value(metric, option, option.label());
                                }
                            }
                        });
                });
            }

            ui.add_space(10.0);

            ui.horizontal(|ui| {
                if ui
                    .add_enabled(self.recorder.is_none(), egui::Button::new("清空实时数据"))
                    .clicked()
                {
                    self.clear_data_confirmation = true;
                }
                if ui.button("恢复图表").clicked() {
                    self.cursor_readout = None;
                    self.reset_plots_requested = true;
                }
            });

            ui.add_space(20.0);
            ui.separator();
            ui.heading("录制与导出");
            ui.separator();

            ui.add_enabled_ui(self.recorder.is_none(), |ui| {
                ui.horizontal(|ui| {
                    ui.label("格式");
                    egui::ComboBox::from_id_salt("recording_format")
                        .selected_text(self.recording_format.label())
                        .show_ui(ui, |ui| {
                            for format in RecordingFormat::ALL {
                                ui.selectable_value(&mut self.recording_format, format, format.label());
                            }
                        });
                });
            });

            match &self.recorder {
                Some(recorder) if recorder.is_finishing() => {
                    ui.add_enabled(false, egui::Button::new("正在安全结束…"));
                }
                Some(_) if self.recording_session => {
                    ui.horizontal(|ui| {
                        let toggle_label = if self.recording_paused {
                            "继续录制"
                        } else {
                            "暂停录制"
                        };
                        if ui.button(toggle_label).clicked() {
                            if self.recording_paused {
                                self.resume_recording();
                            } else {
                                self.pause_recording();
                            }
                        }
                        if ui.button("保存录制").clicked() {
                            self.stop_recording();
                        }
                    });
                }
                Some(_) => {
                    ui.add_enabled(false, egui::Button::new("正在导出…"));
                }
                None => {
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(self.streaming, egui::Button::new("开始录制"))
                            .clicked()
                        {
                            self.start_recording();
                        }
                        if ui
                            .add_enabled(!self.data_points.is_empty(), egui::Button::new("导出缓冲区"))
                            .clicked()
                        {
                            self.export_buffer();
                        }
                    });
                }
            }

            if let Some(recorder) = &self.recorder {
                ui.label(format!("采样点：{}", recorder.rows));
                ui.label(format!("插值：{}", recorder.missing_samples));
                ui.label(format!("丢弃：{}", recorder.discarded_sequence_samples));
                if self.recording_session {
                    ui.label(format!(
                        "录制时长：{}",
                        format_recording_duration(self.displayed_recording_duration())
                    ));
                    ui.label(format!(
                        "累计能量：{}",
                        format_cumulative_energy(self.displayed_cumulative_energy_uwh())
                    ));
                    ui.label(format!(
                        "累计容量：{}",
                        format_capacity(self.displayed_recording_capacity_uah())
                    ));
                    ui.label(format!(
                        "净能量：{}",
                        EnergyPresentation::for_values([self.displayed_recording_net_energy_uwh()])
                            .format_directional(self.displayed_recording_net_energy_uwh())
                    ));
                }
                let completeness = if recorder.elapsed_us == 0 {
                    100.0
                } else {
                    (1.0 - recorder.interpolated_duration_us as f64 / recorder.elapsed_us as f64).max(0.0)
                        * 100.0
                };
                ui.label(format!("数据完整度：{completeness:.6}%"));
            } else if let Some(summary) = &self.last_recording {
                ui.label(format!("上次录制：{} 个采样点", summary.rows));
                ui.label(format!("丢弃：{}", summary.discarded_sequence_samples));
                ui.label(format!(
                    "录制时长：{}",
                    format_recording_duration(self.displayed_recording_duration())
                ));
                ui.label(format!(
                    "累计能量：{}",
                    format_cumulative_energy(self.displayed_cumulative_energy_uwh())
                ));
                ui.label(format!(
                    "累计容量：{}",
                    format_capacity(self.displayed_recording_capacity_uah())
                ));
                ui.label(format!(
                    "净能量：{}",
                    EnergyPresentation::for_values([self.displayed_recording_net_energy_uwh()])
                        .format_directional(self.displayed_recording_net_energy_uwh())
                ));
                ui.label(format!("数据完整度：{:.6}%", summary.completeness_percent()));
            }
            ui.small(&self.recording_status);

            ui.add_space(20.0);
            ui.separator();
            ui.heading("设备离线记录");
            ui.separator();

            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        self.device_state.is_some()
                            && !self.offline_busy
                            && self.recorder.is_none()
                            && self.offline_export.is_none(),
                        egui::Button::new("刷新目录"),
                    )
                    .clicked()
                {
                    self.request_offline_catalog();
                }
                if self.offline_busy {
                    ui.spinner();
                }
            });

            if !self.offline_catalog.is_empty() {
                let selected_text = self
                    .offline_selected
                    .and_then(|index| self.offline_catalog.get(index))
                    .map_or_else(|| "选择一条记录".to_string(), |metadata| metadata.filename_lossy().into_owned());
                egui::ComboBox::from_id_salt("offline_recording")
                    .selected_text(selected_text)
                    .show_ui(ui, |ui| {
                        for (index, metadata) in self.offline_catalog.iter().enumerate() {
                            ui.selectable_value(
                                &mut self.offline_selected,
                                Some(index),
                                format!(
                                    "#{} {}（{} 个采样点）",
                                    index,
                                    metadata.filename_lossy(),
                                    metadata.sample_count
                                ),
                            );
                        }
                    });

                if let Some(metadata) = self
                    .offline_selected
                    .and_then(|index| self.offline_catalog.get(index))
                {
                    egui::Grid::new("offline_metadata_grid")
                        .num_columns(2)
                        .spacing([10.0, 4.0])
                        .show(ui, |ui| {
                            ui.label("采样点");
                            ui.label(metadata.sample_count.to_string());
                            ui.end_row();
                            ui.label("间隔");
                            ui.label(format!("{} ms", metadata.interval.get::<millisecond>()));
                            ui.end_row();
                            ui.label("时长");
                            ui.label(format!("{:.1} s", metadata.recorded_duration.get::<second>()));
                            ui.end_row();
                            ui.label("最终电荷");
                            ui.label(format!("{:.3} mAh", metadata.final_charge.get::<milliampere_hour>()));
                            ui.end_row();
                            ui.label("最终能量");
                            ui.label(format!("{:.3} mWh", metadata.final_energy.get::<milliwatt_hour>()));
                            ui.end_row();
                        });
                }

                if ui
                    .add_enabled(
                        self.device_state.is_some()
                            && self.offline_selected.is_some()
                            && !self.offline_busy
                            && self.recorder.is_none()
                            && self.offline_export.is_none(),
                        egui::Button::new("下载并查看"),
                    )
                    .clicked()
                {
                    self.download_selected_offline_log();
                }
            }

            if let Some(view) = &self.offline_view {
                ui.label(format!(
                    "已载入：{}（{} 个采样点）",
                    view.log.metadata.filename_lossy(),
                    view.samples.len()
                ));
                let previous_source = self.plot_source;
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.plot_source, PlotSource::Live, "查看实时");
                    ui.selectable_value(&mut self.plot_source, PlotSource::Offline, "查看离线");
                });
                if previous_source != self.plot_source {
                    self.cursor_readout = None;
                    self.reset_plots_requested = true;
                    if self.plot_source == PlotSource::Offline {
                        self.time_window = TimeWindow::All;
                        for metric in &mut self.plot_metrics {
                            if !metric.supports_offline() {
                                *metric = PlotMetric::Voltage;
                            }
                        }
                    }
                }
                ui.add_enabled_ui(self.offline_export.is_none() && self.recorder.is_none(), |ui| {
                    ui.horizontal(|ui| {
                        ui.label("导出格式");
                        egui::ComboBox::from_id_salt("offline_recording_format")
                            .selected_text(self.recording_format.label())
                            .show_ui(ui, |ui| {
                                for format in RecordingFormat::ALL {
                                    ui.selectable_value(&mut self.recording_format, format, format.label());
                                }
                            });
                    });
                });
                if let Some(export) = &self.offline_export {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(format!("正在导出 {}", export.path.display()));
                    });
                } else if ui.button("导出已下载记录").clicked() {
                    self.export_offline_log();
                }
            }
            ui.small(&self.offline_status);

            ui.add_space(12.0);
            ui.separator();
            ui.collapsing("关于 KM003C 工作台", |ui| {
                ui.small(format!("版本 v{APP_VERSION}（构建 {APP_BUILD}）"));
                ui.small("基于开源 km003c-rs，遵循 MIT / Apache-2.0 许可。");
                ui.small("本软件不是 ChargerLAB 官方软件。");
                ui.small("日志目录：~/Library/Application Support/com.weixun.km003cworkbench/logs/");
            });

            ui.add_space(5.0);

            if self.streaming {
                if ui.button("断开连接").clicked() {
                    info!("Disconnect requested");
                    self.disconnect_requested = true;
                    let _ = self.cmd_sender.send(UsbCommand::Disconnect);
                }
            } else if self.device_state.is_none() {
                ui.checkbox(&mut self.usb_reset, "连接时手动 USB reset（高级）");
                if ui.button("连接设备").clicked() {
                    info!("Connect requested");
                    let _ = self
                        .cmd_sender
                        .send(UsbCommand::Connect(self.selected_rate.to_graph_rate(), self.usb_reset));
                }
            }
            });

        });

            // Bottom panel with the combined PD timeline
            if self.pd_panel_visible {
                egui::Panel::bottom("pd_panel")
                    .resizable(true)
                    .min_size(100.0)
                    .default_size(200.0)
                    .show(ui, |ui| {
                        ui.heading("USB PD 时间线");
                        if self.pd_trace_enabled {
                            ui.small("[FW] 时间戳精度为 1 秒；同一秒内与 [WIRE] 报文的先后顺序为近似值。");
                        }
                        ui.separator();

                        let text_style = egui::TextStyle::Monospace;
                        let row_height = ui.text_style_height(&text_style);
                        let timeline = pd_timeline_entries(
                            &self.pd_log,
                            &self.pd_trace_log,
                            self.pd_protocol_visible,
                            self.pd_trace_enabled,
                        );

                        egui::ScrollArea::vertical()
                            .auto_shrink([false; 2])
                            .stick_to_bottom(self.pd_auto_scroll)
                            .show(ui, |ui| {
                                for timeline_entry in timeline {
                                    match timeline_entry {
                                        PdTimelineEntry::Protocol(entry) => {
                                            let color = match entry.category {
                                                PdCategory::Connect => egui::Color32::GREEN,
                                                PdCategory::Disconnect => egui::Color32::RED,
                                                PdCategory::SourceCaps => egui::Color32::from_rgb(100, 149, 237),
                                                PdCategory::Request => egui::Color32::YELLOW,
                                                PdCategory::Contract => egui::Color32::LIGHT_GREEN,
                                                PdCategory::Control => egui::Color32::GRAY,
                                                PdCategory::Extended => egui::Color32::from_rgb(255, 165, 0),
                                                PdCategory::Error => egui::Color32::from_rgb(255, 80, 80),
                                            };

                                            ui.colored_label(
                                                color,
                                                egui::RichText::new(format!("[WIRE] {}", entry.summary))
                                                    .monospace()
                                                    .size(row_height),
                                            );
                                            for detail in &entry.details {
                                                ui.colored_label(
                                                    color.gamma_multiply(0.8),
                                                    egui::RichText::new(format!("       {detail}"))
                                                        .monospace()
                                                        .size(row_height),
                                                );
                                            }
                                        }
                                        PdTimelineEntry::FirmwareTrace(entry) => {
                                            let color = match entry.category {
                                                PdTraceCategory::TypeCState => egui::Color32::from_rgb(100, 200, 255),
                                                PdTraceCategory::ProtocolEvent => egui::Color32::LIGHT_GREEN,
                                                PdTraceCategory::Unknown => egui::Color32::YELLOW,
                                            };
                                            ui.colored_label(
                                                color,
                                                egui::RichText::new(format!("[FW]   {}", entry.summary))
                                                    .monospace()
                                                    .size(row_height),
                                            );
                                        }
                                    }
                                }
                            });
                    });
            }

            // Main panel with plots
            egui::CentralPanel::default().show(ui, |ui| {
                let current_time = match self.plot_source {
                    PlotSource::Live => self.data_points.back().map_or(0.0, |sample| sample.elapsed_seconds()),
                    PlotSource::Offline => self
                        .offline_view
                        .as_ref()
                        .and_then(|view| view.samples.last())
                        .map_or(0.0, |sample| sample.elapsed_seconds()),
                };
                if let Some(readout) = self.cursor_readout.or_else(|| self.cursor_readout_at(current_time)) {
                    let cursor_is_pinned = self.cursor_readout.is_some();
                    self.show_cursor_strip(ui, readout, cursor_is_pinned);
                    if ui
                        .add_enabled(cursor_is_pinned, egui::Button::new("跟随最新").small())
                        .clicked()
                    {
                        self.cursor_readout = None;
                        self.reset_plots_requested = true;
                    }
                    ui.add_space(4.0);
                }
                match self.plot_source {
                    PlotSource::Live => ui.small("数据源：实时 AdcQueue"),
                    PlotSource::Offline => {
                        let filename = self
                            .offline_view
                            .as_ref()
                            .map_or_else(|| "未加载".into(), |view| view.log.metadata.filename_lossy());
                        ui.small(format!("数据源：设备离线记录 {filename}"))
                    }
                };
                let available_height = ui.available_height();
                let plot_height = (available_height - 30.0) / 3.0;

                let min_time = self
                    .time_window
                    .seconds()
                    .map(|window| (current_time - window).max(0.0));
                let max_plot_points = (ui.available_width().max(256.0) * 2.0) as usize;

                let mut next_cursor_time = None;
                for (index, metric) in self.plot_metrics.into_iter().enumerate() {
                    ui.label(format!("{} ({})", metric.label(), metric.unit()));
                    let mut plot = Plot::new(("measurement_plot", index))
                        .height(plot_height)
                        .show_axes([true, true])
                        .show_grid(true)
                        .link_axis("measurement-axis", [true, false])
                        .link_cursor("measurement-cursor", [true, false])
                        .show_crosshair(true)
                        .allow_boxed_zoom(true)
                        .allow_drag(true)
                        .allow_scroll(true);
                    if self.reset_plots_requested {
                        plot = plot.reset();
                    }
                    let response = plot.show(ui, |plot_ui| {
                        let raw_points: Vec<[f64; 2]> = match self.plot_source {
                            PlotSource::Live => self
                                .data_points
                                .iter()
                                .filter(|sample| min_time.is_none_or(|min| sample.elapsed_seconds() >= min))
                                .map(|sample| [sample.elapsed_seconds(), metric.value(sample)])
                                .collect(),
                            PlotSource::Offline => self
                                .offline_view
                                .iter()
                                .flat_map(|view| &view.samples)
                                .filter(|sample| min_time.is_none_or(|min| sample.elapsed_seconds() >= min))
                                .filter_map(|sample| {
                                    sample
                                        .metric_value(metric)
                                        .map(|value| [sample.elapsed_seconds(), value])
                                })
                                .collect(),
                        };
                        let points: PlotPoints = min_max_downsample(raw_points, max_plot_points).into();
                        plot_ui.line(Line::new(metric.label(), points).color(metric.color()).width(1.6_f32));
                        plot_ui
                            .response()
                            .hovered()
                            .then(|| plot_ui.pointer_coordinate().map(|point| point.x))
                            .flatten()
                    });
                    if let Some(time) = response.inner {
                        next_cursor_time = Some(time);
                        if let Some(readout) = self.cursor_readout_at(time) {
                            response
                                .response
                                .on_hover_ui_at_pointer(|ui| self.show_cursor_table(ui, readout));
                        }
                    }
                }
                if let Some(time) = next_cursor_time {
                    self.cursor_readout = self.cursor_readout_at(time);
                }
                self.reset_plots_requested = false;
            });
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, &self.preferences());
    }

    fn persist_egui_memory(&self) -> bool {
        true
    }

    fn on_exit(&mut self) {
        // Recorder::Drop sends Finish and joins its writer thread. Calling it
        // here makes Finder/Command-Q exits flush the 23-column contract.
        self.stop_recording();
        if !self.demo_mode {
            let _ = self.cmd_sender.send(UsbCommand::Disconnect);
        }
    }
}

async fn usb_streaming_task(tx: mpsc::UnboundedSender<UsbMessage>, mut cmd_rx: mpsc::UnboundedReceiver<UsbCommand>) {
    info!("USB task started, waiting for Connect command");

    // Main loop - wait for commands
    loop {
        // Wait for a command (blocking)
        let cmd = match cmd_rx.recv().await {
            Some(cmd) => cmd,
            None => {
                warn!("Command channel closed");
                break;
            }
        };

        match cmd {
            UsbCommand::Connect(initial_rate, usb_reset) => {
                info!("Connect command received, rate={:?}, reset={}", initial_rate, usb_reset);
                run_streaming_session(&tx, &mut cmd_rx, initial_rate, usb_reset).await;
            }
            UsbCommand::SetSampleRate(_)
            | UsbCommand::SetPdTraceEnabled(_)
            | UsbCommand::RequestOfflineCatalog
            | UsbCommand::DownloadOfflineLog(_)
            | UsbCommand::Disconnect => {
                // Ignore these when not connected
                debug!("Ignoring command while disconnected: {:?}", cmd);
            }
        }
    }
}

async fn run_streaming_session(
    tx: &mpsc::UnboundedSender<UsbMessage>,
    cmd_rx: &mut mpsc::UnboundedReceiver<UsbCommand>,
    initial_rate: GraphSampleRate,
    usb_reset: bool,
) {
    // Connect to device with vendor interface (Full mode for AdcQueue)
    let config = if usb_reset {
        DeviceConfig::vendor()
    } else {
        DeviceConfig::vendor().skip_reset()
    };
    let mut device = match KM003C::new(config).await {
        Ok(dev) => dev,
        Err(e) => {
            error!("Failed to connect: {}", e);
            let _ = tx.send(UsbMessage::ConnectionFailed(e.to_string()));
            return;
        }
    };

    // Send device state to UI (always available in Full mode)
    let state = device.state().expect("device in Full mode");
    info!("Connected to {} (FW {})", state.model(), state.firmware_version());

    if !state.adcqueue_enabled {
        error!("AdcQueue not enabled - authentication may have failed");
        let _ = tx.send(UsbMessage::ConnectionFailed("AdcQueue not enabled".to_string()));
        return;
    }

    let _ = tx.send(UsbMessage::Connected(Arc::new(state.clone())));

    // Initial StopGraph to ensure clean state
    info!("Sending initial StopGraph to ensure clean state");
    let _ = device.stop_graph_mode().await;

    // Start streaming
    let mut current_rate = initial_rate;
    if let Err(e) = start_streaming(&mut device, current_rate, tx).await {
        error!("Failed to start streaming: {}", e);
        let _ = tx.send(UsbMessage::Error(format!("Start failed: {}", e)));
        let _ = tx.send(UsbMessage::Disconnected);
        return;
    }

    // Streaming loop - poll for data and handle commands
    let mut error_count = 0;
    let mut pd_trace_enabled = false;
    const MAX_ERRORS: u32 = 10;

    loop {
        // Check for commands from UI (non-blocking)
        match cmd_rx.try_recv() {
            Ok(UsbCommand::SetSampleRate(new_rate)) => {
                if new_rate != current_rate {
                    info!("Changing sample rate to {:?}", new_rate);

                    // Stop current streaming
                    let _ = device.stop_graph_mode().await;
                    let _ = tx.send(UsbMessage::StreamingStopped);

                    // Start with new rate
                    if let Err(e) = start_streaming(&mut device, new_rate, tx).await {
                        error!("Failed to restart streaming: {}", e);
                        let _ = tx.send(UsbMessage::Error(format!("Restart failed: {}", e)));
                        continue;
                    }
                    current_rate = new_rate;
                }
            }
            Ok(UsbCommand::SetPdTraceEnabled(enabled)) => {
                pd_trace_enabled = enabled;
                info!(
                    "Firmware PD trace collection {}",
                    if enabled { "enabled" } else { "disabled" }
                );
            }
            Ok(UsbCommand::RequestOfflineCatalog) => {
                info!("Loading offline recording catalog");
                if let Err(error) = device.stop_graph_mode().await {
                    let _ = tx.send(UsbMessage::OfflineOperationFailed(format!(
                        "Could not pause streaming for offline catalog access: {error}"
                    )));
                    continue;
                }
                let _ = tx.send(UsbMessage::StreamingStopped);
                match device.request_log_metadata().await {
                    Ok(catalog) => {
                        let _ = tx.send(UsbMessage::OfflineCatalog(catalog));
                    }
                    Err(error) => {
                        let _ = tx.send(UsbMessage::OfflineOperationFailed(format!(
                            "Failed to load offline catalog: {error}"
                        )));
                    }
                }
                if let Err(error) = start_streaming(&mut device, current_rate, tx).await {
                    let _ = tx.send(UsbMessage::Error(format!(
                        "Failed to resume streaming after loading offline catalog: {error}"
                    )));
                    break;
                }
            }
            Ok(UsbCommand::DownloadOfflineLog(metadata)) => {
                info!(
                    filename = %metadata.filename_lossy(),
                    samples = metadata.sample_count,
                    "Downloading offline recording"
                );
                if let Err(error) = device.stop_graph_mode().await {
                    let _ = tx.send(UsbMessage::OfflineOperationFailed(format!(
                        "Could not pause streaming for offline download: {error}"
                    )));
                    continue;
                }
                let _ = tx.send(UsbMessage::StreamingStopped);
                match device.download_offline_log(metadata).await {
                    Ok(log) => {
                        let _ = tx.send(UsbMessage::OfflineLogDownloaded(log));
                    }
                    Err(error) => {
                        let _ = tx.send(UsbMessage::OfflineOperationFailed(format!(
                            "Failed to download offline recording: {error}"
                        )));
                    }
                }
                if let Err(error) = start_streaming(&mut device, current_rate, tx).await {
                    let _ = tx.send(UsbMessage::Error(format!(
                        "Failed to resume streaming after offline download: {error}"
                    )));
                    break;
                }
            }
            Ok(UsbCommand::Disconnect) => {
                info!("Disconnect command received");
                break;
            }
            Ok(UsbCommand::Connect(..)) => {
                // Ignore connect while already connected
                debug!("Ignoring Connect while already streaming");
            }
            Err(mpsc::error::TryRecvError::Empty) => {
                // No command, continue polling
            }
            Err(mpsc::error::TryRecvError::Disconnected) => {
                warn!("Command channel disconnected");
                break;
            }
        }

        // Request the regular streams and the opt-in firmware trace.
        let mask = streaming_attribute_mask(pd_trace_enabled);
        match device.request_data(mask).await {
            Ok(packet) => {
                error_count = 0;

                if let Some(queue_data) = packet.get_adc_queue()
                    && !queue_data.samples.is_empty()
                {
                    debug!("Received {} samples", queue_data.samples.len());
                    if tx.send(UsbMessage::Samples(queue_data.samples.clone())).is_err() {
                        warn!("UI closed, stopping");
                        break;
                    }
                }

                if let Some(stream) = packet.get_pd_events() {
                    let _ = tx.send(UsbMessage::PdStatusUpdate(stream.preamble));
                    let _ = tx.send(UsbMessage::PdEvents(stream.events.clone()));
                }
                if let Some(status) = packet.get_pd_status() {
                    let _ = tx.send(UsbMessage::PdStatusUpdate(*status));
                }
                if let Some(trace) = packet.get_pd_trace()
                    && (!trace.state_events.is_empty() || !trace.protocol_events.is_empty())
                {
                    let _ = tx.send(UsbMessage::PdTrace(trace.clone()));
                }
            }
            Err(e) => {
                error_count += 1;
                debug!("Request error: {}", e);
                if error_count >= MAX_ERRORS {
                    let _ = tx.send(UsbMessage::Error("Too many errors".to_string()));
                    break;
                }
            }
        }

        // Small delay between requests - adjust based on sample rate
        let delay_ms = match current_rate {
            GraphSampleRate::Sps2 => 200,  // 5 requests/sec for 2 SPS
            GraphSampleRate::Sps10 => 50,  // 20 requests/sec for 10 SPS
            GraphSampleRate::Sps50 => 20,  // 50 requests/sec for 50 SPS
            GraphSampleRate::Sps1000 => 5, // 200 requests/sec for 1000 SPS
        };
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
    }

    // Stop streaming and disconnect
    info!("Stopping streaming");
    let _ = device.stop_graph_mode().await;
    let _ = tx.send(UsbMessage::Disconnected);
}

fn streaming_attribute_mask(pd_trace_enabled: bool) -> AttributeSet {
    let mask = AttributeSet::single(Attribute::AdcQueue).with(Attribute::PdPacket);
    if pd_trace_enabled {
        mask.with(Attribute::PdTrace)
    } else {
        mask
    }
}

async fn start_streaming(
    device: &mut KM003C,
    rate: GraphSampleRate,
    tx: &mpsc::UnboundedSender<UsbMessage>,
) -> Result<(), km003c_lib::error::KMError> {
    info!("Starting AdcQueue streaming at {:?}", rate);
    device.start_graph_mode(rate).await?;
    let _ = tx.send(UsbMessage::StreamingStarted(rate));
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let demo_mode = std::env::args().any(|arg| arg == "--demo");
    let runtime_app_id = std::env::var("KM003C_NATIVE_APP_ID").unwrap_or_else(|_| APP_ID.to_string());
    let runtime_title = std::env::var("KM003C_WINDOW_TITLE").unwrap_or_else(|_| APP_TITLE.to_string());
    init_logging(&runtime_app_id);
    info!(demo_mode, "Starting KM003C Workbench");

    // Create channels for communication
    let (usb_tx, usb_rx) = mpsc::unbounded_channel();
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();

    // Spawn USB streaming task
    tokio::spawn(usb_streaming_task(usb_tx, cmd_rx));

    // Run egui application
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../../assets/app-icon-master.png")).ok();
    let mut viewport = egui::ViewportBuilder::default()
        .with_app_id(&runtime_app_id)
        .with_inner_size([1280.0, 820.0])
        .with_min_inner_size([1024.0, 700.0])
        .with_title(&runtime_title);
    if let Some(icon) = icon {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        persist_window: true,
        // `persistence_path` is a file, while `storage_dir` is the directory
        // shared by logs and recoverable recordings. Passing the directory
        // itself made every preference save fail with EISDIR on macOS.
        persistence_path: std::env::var_os("KM003C_STORAGE_ROOT")
            .map(PathBuf::from)
            .or_else(|| eframe::storage_dir(&runtime_app_id))
            .map(|path| path.join("preferences.ron")),
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        &runtime_title,
        options,
        Box::new(move |cc| {
            Ok(Box::new(PowerMonitorApp::new_with_context(
                cc, usb_rx, cmd_tx, demo_mode,
            )))
        }),
    )
    .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
}

fn init_logging(runtime_app_id: &str) {
    let log_dir = std::env::var_os("KM003C_STORAGE_ROOT")
        .map(PathBuf::from)
        .or_else(|| eframe::storage_dir(runtime_app_id))
        .map(|path| path.join("logs"));
    if let Some(log_dir) = log_dir
        && std::fs::create_dir_all(&log_dir).is_ok()
        && let Ok(file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_dir.join("session.log"))
    {
        tracing_subscriber::fmt().with_writer(file).with_ansi(false).init();
        return;
    }
    tracing_subscriber::fmt().with_ansi(false).init();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::offline_view::captured_test_view;
    use polars::prelude::{CsvReader, ParquetReader, SerReader};

    #[test]
    fn low_ranges_use_engineering_units_and_keep_distinct_ticks() {
        let current_scale = AxisScale::from_visible_max(0.003);
        assert_eq!(current_scale.maximum, 0.005);
        assert!((current_scale.normalize(0.004) - 0.8).abs() < f64::EPSILON);
        assert!((current_scale.denormalize(0.8) - 0.004).abs() < f64::EPSILON);

        let current_axis = current_scale.presentation(MeasurementUnit::Current);
        assert_eq!(current_axis.symbol, "mA");
        assert_eq!(current_axis.format_value(0.001), "1.00");
        assert_eq!(current_axis.format_value(0.004), "4.00");

        let power_axis = AxisScale::from_visible_max(0.000_04).presentation(MeasurementUnit::Power);
        assert_eq!(power_axis.symbol, "µW");
        assert_ne!(power_axis.format_value(0.000_01), power_axis.format_value(0.000_02));
    }

    #[test]
    fn monitor_layout_compacts_before_the_protocol_and_signal_cards_clip() {
        assert!(!uses_compact_monitor_layout(1280.0, 740.0));
        assert!(uses_compact_monitor_layout(1239.0, 740.0));
        assert!(uses_compact_monitor_layout(1280.0, 619.0));
        assert!(uses_compact_monitor_layout(1024.0, 560.0));
    }

    #[test]
    fn toolbar_density_reserves_space_for_english_controls() {
        assert_eq!(toolbar_density(1440.0), ToolbarDensity::Full);
        assert_eq!(toolbar_density(1160.0), ToolbarDensity::Compact);
        assert_eq!(toolbar_density(1024.0), ToolbarDensity::Narrow);
    }

    #[test]
    fn settings_navigation_has_complete_bilingual_labels() {
        for page in SettingsPage::ALL {
            assert!(!page.localized_label(Language::SimplifiedChinese).is_empty());
            assert!(!page.localized_label(Language::English).is_empty());
            assert!(!page.localized_description(Language::SimplifiedChinese).is_empty());
            assert!(!page.localized_description(Language::English).is_empty());
        }
        assert_eq!(Language::SimplifiedChinese.short_name(), "简中");
        assert_eq!(Language::English.short_name(), "EN");
    }

    #[test]
    fn settings_layout_keeps_navigation_content_and_footer_in_bounds() {
        for viewport_size in [
            egui::vec2(1024.0, 700.0),
            egui::vec2(1280.0, 820.0),
            egui::vec2(1728.0, 1117.0),
        ] {
            let content_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, viewport_size);
            let metrics = SettingsLayoutMetrics::for_content_rect(content_rect);

            assert!(metrics.window_size.x <= viewport_size.x - 24.0);
            assert!(metrics.window_size.y <= viewport_size.y - 24.0);
            assert!(
                (SettingsLayoutMetrics::MIN_WINDOW_WIDTH..=SettingsLayoutMetrics::MAX_WINDOW_WIDTH)
                    .contains(&metrics.window_size.x)
            );
            assert!(
                (SettingsLayoutMetrics::MIN_WINDOW_HEIGHT..=SettingsLayoutMetrics::MAX_WINDOW_HEIGHT)
                    .contains(&metrics.window_size.y)
            );
            assert_eq!(metrics.navigation_width, 184.0);
            assert!(metrics.content_width >= 580.0);
            assert!(metrics.scroll_height >= 400.0);
            assert_eq!(metrics.footer_height, 44.0);
        }
    }

    #[test]
    fn settings_form_columns_leave_room_for_both_languages() {
        assert_eq!(SETTINGS_FORM_LABEL_WIDTH, 148.0);
        assert_eq!(settings_control_width(680.0), 360.0);
        assert_eq!(settings_control_width(400.0), 240.0);
    }

    #[test]
    fn recoverable_file_rows_share_one_fixed_action_column() {
        assert_eq!(recoverable_file_columns(680.0), (598.0, 72.0));
        assert_eq!(recoverable_file_columns(202.0), (120.0, 72.0));

        let (details, actions) = recoverable_session_columns(680.0);
        assert_eq!(actions, 244.0);
        assert_eq!(details, 424.0);
    }

    #[test]
    fn recoverable_session_states_have_complete_bilingual_labels() {
        for state in [
            SessionState::Recording,
            SessionState::Paused,
            SessionState::WaitingForReconnect,
            SessionState::Finalizing,
            SessionState::Saved,
            SessionState::Interrupted,
        ] {
            assert!(!localized_session_state(state, Language::SimplifiedChinese).is_empty());
            assert!(!localized_session_state(state, Language::English).is_empty());
        }
        assert_eq!(
            localized_session_state(SessionState::Interrupted, Language::SimplifiedChinese),
            "异常中断"
        );
    }

    #[test]
    fn navigator_selection_clamps_without_losing_its_range() {
        let selection = NavigatorSelection {
            start_seconds: 8.0,
            end_seconds: 14.0,
        }
        .clamped(10.0);
        assert_eq!(selection.start_seconds, 4.0);
        assert_eq!(selection.end_seconds, 10.0);
    }

    #[test]
    fn display_filter_rejects_single_sample_spikes_without_touching_raw_points() {
        let points = vec![[0.0, 1.0], [1.0, 1.0], [2.0, 99.0], [3.0, 1.0], [4.0, 1.0]];
        let raw = apply_display_filter(points.clone(), DisplayFilter::Raw);
        let filtered = apply_display_filter(points.clone(), DisplayFilter::Median5);
        assert_eq!(raw, points);
        assert_eq!(filtered[2], [2.0, 1.0]);
        assert_eq!(filtered.first(), Some(&[0.0, 1.0]));
        assert_eq!(filtered.last(), Some(&[4.0, 1.0]));
    }

    #[test]
    fn older_three_point_filter_preference_migrates_to_the_new_default() {
        let restored: DisplayFilter = serde_json::from_str("\"Median3\"").unwrap();
        assert_eq!(restored, DisplayFilter::Median5);
    }

    #[test]
    fn live_waveform_is_only_visible_during_an_explicit_recording() {
        let (usb_tx, usb_rx) = mpsc::unbounded_channel();
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel();
        let mut app = PowerMonitorApp::new(usb_rx, cmd_tx);
        app.data_points.push_back(test_measurement(0.0));
        assert!(!app.monitor_chart_visible());

        app.recording_session = true;
        assert!(app.monitor_chart_visible());
        app.recording_session = false;
        app.last_recording_duration = Some(Duration::from_secs(10));
        assert!(!app.monitor_chart_visible());

        app.plot_source = PlotSource::Offline;
        app.offline_view = Some(Arc::new(captured_test_view()));
        assert!(app.monitor_chart_visible());
        drop(usb_tx);
    }

    #[test]
    fn window_statistics_exclude_pause_intervals() {
        let samples = (0..=5)
            .map(|second| {
                let mut sample = test_measurement(second as f64 * 10.0);
                sample.elapsed_us = second * 1_000_000;
                sample.sample_index = second;
                sample.charge_throughput_uah = second as f64;
                sample
            })
            .collect::<Vec<_>>();
        let statistics = calculate_scope_statistics(
            samples.iter().map(|sample| (sample.elapsed_seconds(), sample)),
            NavigatorSelection {
                start_seconds: 1.0,
                end_seconds: 5.0,
            },
            &[PauseInterval {
                start_seconds: 2.0,
                end_seconds: 3.0,
            }],
        );
        assert_eq!(statistics.points, 3);
        assert_eq!(statistics.duration_seconds, 1.0);
        assert_eq!(statistics.cumulative_energy_uwh, 10.0);
        assert_eq!(statistics.capacity_uah, 1.0);
    }

    #[test]
    fn navigator_history_compacts_but_keeps_the_whole_time_span() {
        let mut history = NavigatorHistory::with_limit(4);
        for index in 0..40 {
            let time = index as f64 * 0.11;
            history.push_values(time, [9.0, 1.0 + index as f64, 9.0 + index as f64]);
        }
        assert!(history.points.len() <= 4);
        assert!(history.bucket_width_seconds > 0.1);
        assert_eq!(history.points.last().unwrap().time_seconds, 39.0 * 0.11);
        assert!(history.points.first().unwrap().time_seconds < history.points.last().unwrap().time_seconds);
    }

    #[test]
    fn navigator_history_keeps_short_spikes_after_progressive_compaction() {
        let mut history = NavigatorHistory::with_limit(4);
        for index in 0..40 {
            let spike = if index == 17 { 99.0 } else { 1.0 };
            history.push_values(index as f64 * 0.11, [9.0, spike, 9.0 * spike]);
        }
        assert!(history.points.iter().any(|point| point.maximums[1] == 99.0));

        let (usb_tx, usb_rx) = mpsc::unbounded_channel();
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel();
        let mut app = PowerMonitorApp::new(usb_rx, cmd_tx);
        app.navigator_history = history;
        let overview = app.navigator_vip_points(100);
        assert!(overview[1].iter().any(|point| point[1] == 99.0));
        drop(usb_tx);
    }

    #[test]
    fn recording_full_session_follow_uses_a_zero_origin_and_expands_each_frame() {
        let (usb_tx, usb_rx) = mpsc::unbounded_channel();
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel();
        let mut app = PowerMonitorApp::new(usb_rx, cmd_tx);
        let mut origin = test_measurement(0.0);
        origin.elapsed_us = 80_000_000;
        let mut later = test_measurement(1.0);
        later.elapsed_us = 100_000_000;
        app.data_points.extend([origin, later]);
        app.live_plot_origin_seconds = 80.0;
        app.recording_session = true;
        app.chart_follow_mode = ChartFollowMode::FullSession;

        assert_eq!(app.source_end_time(), 20.0);
        let first_selection = app.ensure_chart_selection();
        assert_eq!(first_selection.start_seconds, 0.0);
        assert_eq!(first_selection.end_seconds, 20.0);
        assert_eq!(app.cursor_readout_at(0.0).unwrap().time_seconds, 0.0);

        let mut newest = test_measurement(2.0);
        newest.elapsed_us = 110_000_000;
        app.data_points.push_back(newest);
        let expanded = app.ensure_chart_selection();
        assert_eq!(expanded.start_seconds, 0.0);
        assert_eq!(expanded.end_seconds, 30.0);

        app.chart_follow_mode = ChartFollowMode::Manual;
        app.chart_viewport.selection = Some(NavigatorSelection {
            start_seconds: 5.0,
            end_seconds: 12.0,
        });
        let mut while_reviewing = test_measurement(3.0);
        while_reviewing.elapsed_us = 120_000_000;
        app.data_points.push_back(while_reviewing);
        assert_eq!(
            app.ensure_chart_selection(),
            NavigatorSelection {
                start_seconds: 5.0,
                end_seconds: 12.0,
            }
        );

        app.resume_chart_following();
        assert_eq!(app.chart_follow_mode, ChartFollowMode::LatestWindow);
        assert_eq!(app.ensure_chart_selection().end_seconds, 40.0);
        drop(usb_tx);
    }

    #[test]
    fn firmware_trace_is_only_requested_when_enabled() {
        let disabled = streaming_attribute_mask(false);
        assert!(disabled.contains(Attribute::AdcQueue));
        assert!(disabled.contains(Attribute::PdPacket));
        assert!(!disabled.contains(Attribute::PdTrace));

        let enabled = streaming_attribute_mask(true);
        assert!(enabled.contains(Attribute::AdcQueue));
        assert!(enabled.contains(Attribute::PdPacket));
        assert!(enabled.contains(Attribute::PdTrace));
    }

    #[test]
    fn pd_timeline_filters_and_orders_both_sources() {
        let protocol_log = VecDeque::from([DecodedPdEntry {
            timestamp_seconds: 12.25,
            category: PdCategory::Control,
            summary: "wire".to_string(),
            details: Vec::new(),
        }]);
        let trace_log = VecDeque::from([PdTraceEntry {
            timestamp_seconds: 11.0,
            category: PdTraceCategory::TypeCState,
            summary: "trace".to_string(),
        }]);

        let combined = pd_timeline_entries(&protocol_log, &trace_log, true, true);
        assert!(matches!(combined[0], PdTimelineEntry::FirmwareTrace(_)));
        assert!(matches!(combined[1], PdTimelineEntry::Protocol(_)));

        let protocol_only = pd_timeline_entries(&protocol_log, &trace_log, true, false);
        assert_eq!(protocol_only.len(), 1);
        assert!(matches!(protocol_only[0], PdTimelineEntry::Protocol(_)));
    }

    #[test]
    fn downloaded_offline_log_becomes_the_active_plot_source() {
        let (usb_tx, usb_rx) = mpsc::unbounded_channel();
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel();
        let mut app = PowerMonitorApp::new(usb_rx, cmd_tx);
        app.plot_metrics[0] = PlotMetric::Cc1;
        let fixture = captured_test_view();

        usb_tx
            .send(UsbMessage::OfflineLogDownloaded(fixture.log.as_ref().clone()))
            .unwrap();
        app.process_messages();

        assert_eq!(app.plot_source, PlotSource::Offline);
        assert_eq!(app.time_window, TimeWindow::All);
        assert_eq!(app.plot_metrics[0], PlotMetric::Voltage);
        assert_eq!(app.offline_view.as_ref().unwrap().samples.len(), 3);
        assert!(!app.offline_busy);
    }

    #[test]
    fn empty_offline_catalog_clears_selection() {
        let (usb_tx, usb_rx) = mpsc::unbounded_channel();
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel();
        let mut app = PowerMonitorApp::new(usb_rx, cmd_tx);
        app.offline_selected = Some(2);

        usb_tx.send(UsbMessage::OfflineCatalog(Vec::new())).unwrap();
        app.process_messages();

        assert!(app.offline_catalog.is_empty());
        assert_eq!(app.offline_selected, None);
        assert!(app.offline_status.contains("没有离线记录"));
    }

    #[test]
    fn connection_error_mapping_only_retries_missing_devices() {
        let (phase, message) = i18n::connection_error(Language::SimplifiedChinese, "Device not found");
        assert_eq!(phase, ConnectionPhase::NoDevice);
        assert!(message.contains("自动重试"));

        let (phase, message) = i18n::connection_error(Language::SimplifiedChinese, "resource busy");
        assert_eq!(phase, ConnectionPhase::DeviceBusy);
        assert!(message.contains("关闭其它"));
        assert!(phase.is_terminal_failure());
    }

    #[test]
    fn preferences_keep_only_user_interface_choices() {
        let (usb_tx, usb_rx) = mpsc::unbounded_channel();
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel();
        let mut app = PowerMonitorApp::new(usb_rx, cmd_tx);
        app.selected_rate = SampleRateOption::Sps1000;
        app.time_window = TimeWindow::Min5;
        app.pd_panel_visible = false;
        app.auto_pause_enabled = true;
        app.auto_pause_threshold_mw = 250;
        app.auto_pause_delay_ms = 4_500;
        app.display_filter = DisplayFilter::Raw;
        let prefs = app.preferences();
        assert_eq!(prefs.selected_rate, SampleRateOption::Sps1000);
        assert_eq!(prefs.time_window, TimeWindow::Min5);
        assert!(!prefs.pd_panel_visible);
        assert!(prefs.auto_pause_enabled);
        assert_eq!(prefs.auto_pause_threshold_mw, 250);
        assert_eq!(prefs.auto_pause_delay_ms, 4_500);
        assert_eq!(prefs.display_filter, DisplayFilter::Raw);
        assert!(prefs.plot_metrics.iter().all(|metric| metric.supports_offline()));
        let restored: AppPreferences = serde_json::from_str(&serde_json::to_string(&prefs).unwrap()).unwrap();
        assert_eq!(restored, prefs);
        drop(usb_tx);
    }

    #[test]
    fn preferences_from_older_build_receive_safe_auto_pause_defaults() {
        let mut value = serde_json::to_value(AppPreferences::default()).unwrap();
        let object = value.as_object_mut().expect("preferences serialize as an object");
        object.remove("auto_pause_enabled");
        object.remove("auto_pause_threshold_mw");
        object.remove("auto_pause_delay_ms");
        object.remove("display_filter");
        let restored: AppPreferences = serde_json::from_value(value).unwrap();
        assert!(!restored.auto_pause_enabled);
        assert_eq!(restored.auto_pause_threshold_mw, 100);
        assert_eq!(restored.auto_pause_delay_ms, 3_000);
        assert_eq!(restored.display_filter, DisplayFilter::Median5);
    }

    #[test]
    fn demo_mode_has_obvious_deterministic_measurements() {
        let (usb_tx, usb_rx) = mpsc::unbounded_channel();
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel();
        let mut app = PowerMonitorApp::with_defaults(usb_rx, cmd_tx, true);
        app.enable_demo_device();
        app.demo_started = Instant::now() - Duration::from_secs(10);
        app.demo_last_tick = Instant::now() - Duration::from_secs(1);
        app.update_demo_data();
        assert!(app.demo_mode);
        assert_eq!(app.phase, ConnectionPhase::Streaming);
        assert!(app.status.contains("演示数据"));
        assert_eq!(app.data_points.len(), 1);
        assert!(app.current_voltage > 8.0 && app.current_voltage < 10.0);
        let sample = app.data_points.back().unwrap();
        assert!(sample.charge_throughput_uah > 3_000.0);
        assert!(sample.energy_throughput_uwh > 25_000.0);
        drop(usb_tx);
    }

    #[test]
    fn live_vip_readouts_do_not_require_a_recording() {
        let (usb_tx, usb_rx) = mpsc::unbounded_channel();
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel();
        let mut app = PowerMonitorApp::new(usb_rx, cmd_tx);
        app.streaming = true;
        app.phase = ConnectionPhase::Streaming;
        let sample = test_measurement(0.0);

        app.append_measurements(&[sample]);

        assert!(!app.recording_session);
        assert_eq!(app.instrument_readout_status(), "实时");
        let readout = app.displayed_current_readout();
        assert_eq!(readout.voltage, sample.vbus_uv as f64 / 1_000_000.0);
        assert_eq!(readout.current, (sample.ibus_ua as f64 / 1_000_000.0).abs());
        assert_eq!(readout.power, (sample.power_uw as f64 / 1_000_000.0).abs());

        app.streaming = false;
        app.phase = ConnectionPhase::Disconnected;
        assert_eq!(app.instrument_readout_status(), "最后读数");
        drop(usb_tx);
    }

    #[test]
    fn recording_summary_formats_match_instrument_toolbar() {
        assert_eq!(format_recording_duration(Duration::from_millis(6_800)), "00:00:06.8");
        assert_eq!(format_recording_duration(Duration::from_secs(3_661)), "01:01:01.0");
        assert_eq!(format_cumulative_energy(17_300.0), "17.300 mWh");
        assert_eq!(format_cumulative_energy(1_234_567.0), "1.2346 Wh");
        assert_eq!(format_capacity(93_070.0), "93.070 mAh");
        let energy = EnergyPresentation::for_values([-856_100.0]);
        assert_eq!(energy.format_directional(-856_100.0), "↓ 856.100 mWh");
        let shared_energy = EnergyPresentation::for_values([17_300.0, -856_100.0]);
        assert_eq!(shared_energy.format(17_300.0), "17.300 mWh");
        assert_eq!(shared_energy.format_directional(-856_100.0), "↓ 856.100 mWh");
    }

    #[test]
    fn running_statistics_report_minimum_average_and_maximum() {
        let mut statistics = RunningMetricStatistics::default();
        statistics.push(9.0);
        statistics.push(12.0);
        statistics.push(6.0);
        let readout = statistics.readout().expect("statistics should contain values");
        assert_eq!(readout.minimum, 6.0);
        assert_eq!(readout.average, 9.0);
        assert_eq!(readout.maximum, 12.0);
    }

    #[test]
    fn nearest_time_lookup_uses_sorted_data_and_prefers_earlier_ties() {
        let times = [0.0, 1.0, 2.0, 3.0, 4.0];
        assert_eq!(nearest_time_index(times.len(), 2.6, |index| times[index]), Some(3));
        assert_eq!(nearest_time_index(times.len(), 1.5, |index| times[index]), Some(1));
        assert_eq!(nearest_time_index(times.len(), 99.0, |index| times[index]), Some(4));
        assert_eq!(nearest_time_index(0, 0.0, |_| 0.0), None);
    }

    #[test]
    fn pixel_downsampling_preserves_endpoints_and_short_spikes() {
        let mut points = (0..1_000).map(|index| [index as f64, 1.0]).collect::<Vec<_>>();
        points[501][1] = 99.0;
        let reduced = min_max_downsample(points, 100);
        assert!(reduced.len() <= 100);
        assert_eq!(reduced.first(), Some(&[0.0, 1.0]));
        assert_eq!(reduced.last(), Some(&[999.0, 1.0]));
        assert!(reduced.contains(&[501.0, 99.0]));
        assert!(reduced.windows(2).all(|pair| pair[0][0] <= pair[1][0]));
    }

    #[test]
    fn auto_pause_requires_sustained_low_power_and_keeps_session_resumable() {
        let (usb_tx, usb_rx) = mpsc::unbounded_channel();
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel();
        let mut app = PowerMonitorApp::new(usb_rx, cmd_tx);
        let recording_path = std::env::temp_dir().join(format!(
            "km003c-auto-pause-{}-{}.csv",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        app.recorder = Some(
            Recorder::start(
                recording_path.clone(),
                RecordingFormat::Csv,
                RecordingMetadata::default(),
                None,
            )
            .unwrap(),
        );
        app.recording_session = true;
        app.recording_phase = RecordingPhase::Recording;
        app.recording_started_at = Some(Instant::now());
        app.auto_pause_enabled = true;
        app.auto_pause_threshold_mw = 100;
        app.auto_pause_delay_ms = 1_000;

        let mut first = test_measurement(0.0);
        first.elapsed_us = 2_000_000;
        first.power_uw = 50_000;
        app.append_measurements(&[first]);
        assert!(!app.recording_paused);

        let mut later_sample = test_measurement(1.0);
        later_sample.elapsed_us = 3_000_000;
        later_sample.power_uw = 50_000;
        app.append_measurements(&[later_sample]);
        assert!(app.recording_paused);
        assert!(app.recording_status.contains("自动暂停"));
        drop(usb_tx);
        drop(app);
        let _ = std::fs::remove_file(recording_path);
    }

    #[test]
    fn missing_writer_can_never_leave_the_ui_claiming_recording() {
        let (usb_tx, usb_rx) = mpsc::unbounded_channel();
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel();
        let mut app = PowerMonitorApp::new(usb_rx, cmd_tx);
        app.recording_session = true;
        app.recording_phase = RecordingPhase::Recording;
        app.recording_paused = false;
        app.recording_started_at = Some(Instant::now() - Duration::from_secs(25));

        app.poll_recording();

        assert_eq!(app.recording_phase, RecordingPhase::Interrupted);
        assert!(app.recording_paused);
        assert!(app.recording_status.contains("录制器已退出"));
        drop(usb_tx);
    }

    #[test]
    fn recording_clock_reanchors_to_captured_samples_after_ui_suspension() {
        let (usb_tx, usb_rx) = mpsc::unbounded_channel();
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel();
        let mut app = PowerMonitorApp::new(usb_rx, cmd_tx);
        app.recording_session = true;
        app.recording_phase = RecordingPhase::Recording;
        app.recording_elapsed_before_pause = Duration::from_secs(5);
        app.recording_started_at = Some(Instant::now() - Duration::from_secs(30));

        app.sync_recording_clock_to_samples(38_000_000);

        let elapsed = app.recording_elapsed();
        assert!(elapsed >= Duration::from_secs(38));
        assert!(elapsed < Duration::from_millis(38_100));
        drop(usb_tx);
    }

    #[test]
    fn usb_unlock_backlog_is_bounded_per_ui_frame() {
        let (usb_tx, usb_rx) = mpsc::unbounded_channel();
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel();
        let mut app = PowerMonitorApp::new(usb_rx, cmd_tx);
        for index in 0..(MAX_USB_MESSAGES_PER_FRAME + 5) {
            usb_tx
                .send(UsbMessage::ConnectionFailed(format!("queued-{index}")))
                .unwrap();
        }

        assert!(app.process_messages());
        assert!(app.usb_receiver.try_recv().is_ok());
        drop(usb_tx);
    }

    #[test]
    fn cursor_readout_joins_voltage_current_and_power_at_nearest_sample() {
        let (usb_tx, usb_rx) = mpsc::unbounded_channel();
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel();
        let mut app = PowerMonitorApp::new(usb_rx, cmd_tx);
        let mut first = test_measurement(1_000.0);
        first.elapsed_us = 1_000_000;
        let mut later = test_measurement(2_000.0);
        later.elapsed_us = 2_000_000;
        app.data_points.push_back(first);
        app.data_points.push_back(later);

        let readout = app.cursor_readout_at(1.8).expect("cursor should find a sample");
        assert_eq!(readout.time_seconds, 2.0);
        assert_eq!(readout.voltage, 9.0);
        assert_eq!(readout.current, 1.0);
        assert_eq!(readout.power, 9.0);
        assert!(!readout.approximate);

        let accumulated = app
            .accumulated_readout()
            .expect("accumulated metrics should be present");
        assert_eq!(accumulated.cumulative_energy_uwh, 2_000.0);
        assert_eq!(accumulated.capacity_uah, 1.0);
        assert_eq!(accumulated.net_energy_uwh, 2_000.0);
        drop(usb_tx);
    }

    #[test]
    fn cumulative_energy_remains_visible_for_reverse_power_flow() {
        let (usb_tx, usb_rx) = mpsc::unbounded_channel();
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel();
        let mut app = PowerMonitorApp::new(usb_rx, cmd_tx);
        let mut sample = test_measurement(1_066_400.0);
        sample.energy_uwh = -1_066_400.0;
        sample.power_uw = -9_000_000;
        sample.ibus_ua = -1_000_000;
        sample.charge_throughput_uah = 116_622.0;
        app.data_points.push_back(sample);

        let accumulated = app
            .accumulated_readout()
            .expect("reverse-flow sample should still expose cumulative totals");
        assert_eq!(accumulated.cumulative_energy_uwh, 1_066_400.0);
        assert_eq!(accumulated.capacity_uah, 116_622.0);
        assert_eq!(accumulated.net_energy_uwh, -1_066_400.0);
        drop(usb_tx);
    }

    #[test]
    fn evicted_live_samples_remain_visible_as_an_explicit_history_overview() {
        let (usb_tx, usb_rx) = mpsc::unbounded_channel();
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel();
        let mut app = PowerMonitorApp::new(usb_rx, cmd_tx);
        app.navigator_history.push_values(1.0, [5.0, 0.5, 2.5]);
        let mut recent = test_measurement(2_000.0);
        recent.elapsed_us = 10_000_000;
        app.data_points.push_back(recent);

        let historical = app
            .cursor_readout_at(1.0)
            .expect("history cursor should remain available");
        assert!(historical.approximate);
        assert_eq!(historical.time_seconds, 1.0);
        assert_eq!(historical.power, 2.5);

        let points = app.source_vip_points(
            NavigatorSelection {
                start_seconds: 0.0,
                end_seconds: 10.0,
            },
            100,
            DisplayFilter::Raw,
        );
        assert_eq!(points[0].first(), Some(&[1.0, 5.0]));
        assert_eq!(points[0].last(), Some(&[10.0, 9.0]));
        drop(usb_tx);
    }

    #[test]
    fn paused_recording_excludes_samples_and_pause_time_from_totals() {
        let (usb_tx, usb_rx) = mpsc::unbounded_channel();
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel();
        let mut app = PowerMonitorApp::new(usb_rx, cmd_tx);
        app.recording_session = true;
        app.recording_started_at = Some(Instant::now() - Duration::from_secs(2));
        app.recording_energy_origin_uwh = 10.0;
        app.recording_capacity_origin_uah = 1.0;
        app.recording_total_capacity_uah = 4.0;
        app.recording_net_energy_origin_uwh = 8.0;
        app.recording_net_energy_uwh = 12.0;
        let mut first_segment = test_measurement(20.0);
        first_segment.charge_throughput_uah = 5.0;
        app.data_points.push_back(first_segment);
        app.recording_total_energy_uwh = 10.0;

        app.pause_recording();
        assert!(app.recording_paused);
        assert!(app.recording_started_at.is_none());
        assert!(app.recording_elapsed() >= Duration::from_secs(1));
        assert_eq!(app.recording_energy_origin_uwh, 20.0);
        assert_eq!(app.recording_energy_completed_uwh, 10.0);
        assert_eq!(app.recording_capacity_origin_uah, 5.0);
        assert_eq!(app.recording_capacity_completed_uah, 4.0);
        assert_eq!(app.recording_net_energy_origin_uwh, 20.0);
        assert_eq!(app.recording_net_energy_completed_uwh, 12.0);

        // Live plots still receive samples, but the paused segment does not
        // change the saved recording's energy total.
        let mut paused_sample = test_measurement(100.0);
        paused_sample.charge_throughput_uah = 100.0;
        app.append_measurements(&[paused_sample]);
        assert_eq!(app.recording_total_energy_uwh, 10.0);
        assert_eq!(app.recording_total_capacity_uah, 4.0);
        assert_eq!(app.recording_net_energy_uwh, 12.0);

        // Continuing rebases every cumulative origin at the last paused
        // sample, so the next segment adds only post-resume deltas.
        app.rebase_recording_origins_at_latest_sample();
        app.recording_paused = false;
        let mut active_sample = test_measurement(105.0);
        active_sample.charge_throughput_uah = 103.0;
        app.append_measurements(&[active_sample]);
        assert_eq!(app.recording_total_energy_uwh, 15.0);
        assert_eq!(app.recording_total_capacity_uah, 7.0);
        assert_eq!(app.recording_net_energy_uwh, 17.0);
        drop(usb_tx);
    }

    fn test_measurement(energy_throughput_uwh: f64) -> MeasurementSample {
        MeasurementSample {
            elapsed_us: 1_000,
            sample_index: 0,
            sequence: 0,
            marker: 0xD3,
            sample_rate_hz: 50,
            missing_samples: 0,
            gap_duration_us: 0,
            interpolated: false,
            cumulative_missing_samples: 0,
            cumulative_interpolated_duration_us: 0,
            discarded_sequence_samples: 0,
            cumulative_discarded_sequence_samples: 0,
            vbus_uv: 9_000_000,
            ibus_ua: 1_000_000,
            power_uw: 9_000_000,
            charge_uah: 1.0,
            energy_uwh: energy_throughput_uwh,
            charge_throughput_uah: 1.0,
            energy_throughput_uwh,
            cc1_uv: 620_000,
            cc2_uv: 40_000,
            dp_uv: 540_000,
            dm_uv: 510_000,
        }
    }

    #[test]
    #[ignore = "requires a connected KM003C"]
    fn hardware_offline_flow_pauses_and_resumes_streaming() {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let (usb_tx, mut usb_rx) = mpsc::unbounded_channel();
            let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
            let task = tokio::spawn(usb_streaming_task(usb_tx, cmd_rx));
            cmd_tx.send(UsbCommand::Connect(GraphSampleRate::Sps50, false)).unwrap();

            let startup_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
            let mut device_state = None;
            let mut streaming = false;
            while !(device_state.is_some() && streaming) {
                let message = tokio::time::timeout_at(startup_deadline, usb_rx.recv())
                    .await
                    .expect("device did not connect before the deadline")
                    .expect("USB task exited during connection");
                match message {
                    UsbMessage::Connected(state) => device_state = Some(state),
                    UsbMessage::StreamingStarted(GraphSampleRate::Sps50) => streaming = true,
                    _ => {}
                }
            }

            cmd_tx.send(UsbCommand::RequestOfflineCatalog).unwrap();
            let operation_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
            let mut stopped = false;
            let catalog = loop {
                let message = tokio::time::timeout_at(operation_deadline, usb_rx.recv())
                    .await
                    .expect("offline catalog request did not finish before the deadline")
                    .expect("USB task exited during offline catalog request");
                match message {
                    UsbMessage::StreamingStopped => stopped = true,
                    UsbMessage::OfflineCatalog(catalog) => {
                        assert!(stopped, "catalog arrived before streaming was paused");
                        break catalog;
                    }
                    UsbMessage::OfflineOperationFailed(error) => panic!("offline catalog failed: {error}"),
                    _ => {}
                }
            };
            loop {
                let message = tokio::time::timeout_at(operation_deadline, usb_rx.recv())
                    .await
                    .expect("streaming did not resume after catalog request")
                    .expect("USB task exited before streaming resumed");
                if matches!(message, UsbMessage::StreamingStarted(GraphSampleRate::Sps50)) {
                    break;
                }
            }

            let mut downloaded_log = None;
            if let Some(metadata) = catalog.first().cloned() {
                cmd_tx.send(UsbCommand::DownloadOfflineLog(metadata.clone())).unwrap();
                let download_deadline = tokio::time::Instant::now() + Duration::from_secs(20);
                let mut stopped = false;
                let mut downloaded = false;
                let mut resumed = false;
                while !(downloaded && resumed) {
                    let message = tokio::time::timeout_at(download_deadline, usb_rx.recv())
                        .await
                        .expect("offline download did not finish before the deadline")
                        .expect("USB task exited during offline download");
                    match message {
                        UsbMessage::StreamingStopped => stopped = true,
                        UsbMessage::OfflineLogDownloaded(log) => {
                            assert!(stopped, "offline log arrived before streaming was paused");
                            assert_eq!(log.metadata, metadata);
                            assert_eq!(log.samples.len(), usize::from(metadata.sample_count));
                            downloaded_log = Some(log);
                            downloaded = true;
                        }
                        UsbMessage::StreamingStarted(GraphSampleRate::Sps50) => resumed = true,
                        UsbMessage::OfflineOperationFailed(error) => panic!("offline download failed: {error}"),
                        _ => {}
                    }
                }
            }

            cmd_tx.send(UsbCommand::Disconnect).unwrap();
            drop(cmd_tx);
            let _ = tokio::time::timeout(Duration::from_secs(5), task)
                .await
                .expect("USB task did not stop after disconnect");

            if let Some(log) = downloaded_log {
                let device_state = device_state.expect("connected device state was not retained");
                let recording_metadata = RecordingMetadata {
                    model: device_state.info.model.clone(),
                    firmware: device_state.info.fw_version.clone(),
                    serial: device_state.info.serial_id.clone(),
                };
                let expected_rows = log.samples.len();
                let expected_charge_uah = log
                    .metadata
                    .final_charge
                    .get::<km003c_lib::uom::si::electric_charge::microampere_hour>();
                let expected_energy_uwh = log
                    .metadata
                    .final_energy
                    .get::<km003c_lib::uom::si::energy::microwatt_hour>();
                let view = Arc::new(OfflineRecordingView::new(log));
                assert_eq!(view.samples.last().unwrap().charge_uah, expected_charge_uah);
                assert_eq!(view.samples.last().unwrap().energy_uwh, expected_energy_uwh);

                for format in RecordingFormat::ALL {
                    let path = std::env::temp_dir().join(format!(
                        "km003c-egui-hardware-offline-{}.{}",
                        std::process::id(),
                        format.extension()
                    ));
                    let mut export =
                        OfflineExportTask::start(path.clone(), format, recording_metadata.clone(), Arc::clone(&view))
                            .unwrap();
                    let export_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
                    let rows = loop {
                        match export.poll_event() {
                            Some(OfflineExportEvent::Finished { rows, .. }) => break rows,
                            Some(OfflineExportEvent::Failed(error)) => panic!("offline export failed: {error}"),
                            None => {
                                assert!(
                                    tokio::time::Instant::now() < export_deadline,
                                    "offline export did not finish before the deadline"
                                );
                                tokio::time::sleep(Duration::from_millis(10)).await;
                            }
                        }
                    };
                    assert_eq!(rows, expected_rows);
                    let dataframe = match format {
                        RecordingFormat::Parquet => ParquetReader::new(std::fs::File::open(&path).unwrap())
                            .finish()
                            .unwrap(),
                        RecordingFormat::Csv => CsvReader::new(std::fs::File::open(&path).unwrap()).finish().unwrap(),
                    };
                    assert_eq!(dataframe.shape(), (expected_rows, 23));
                    assert_eq!(
                        dataframe
                            .column("charge_uah")
                            .unwrap()
                            .f64()
                            .unwrap()
                            .get(expected_rows - 1),
                        Some(expected_charge_uah)
                    );
                    assert_eq!(
                        dataframe
                            .column("energy_uwh")
                            .unwrap()
                            .f64()
                            .unwrap()
                            .get(expected_rows - 1),
                        Some(expected_energy_uwh)
                    );
                    std::fs::remove_file(path).unwrap();
                }
            }
        });
    }
}
