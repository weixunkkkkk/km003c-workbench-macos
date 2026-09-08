use serde::{Deserialize, Serialize};

use crate::i18n::Language;
use crate::measurement::PlotMetric;
use crate::recording::RecordingFormat;
use crate::{SampleRateOption, TimeWindow};

/// Visual skin selection.  This is deliberately presentation-only: changing
/// it never changes the USB session, recording state, or plotted samples.
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum SkinId {
    /// The original restrained instrument palette.
    #[default]
    Industrial,
    /// A low-saturation navy/cyan palette with the embedded Japanese-style
    /// character wallpaper selected for this workbench.
    CleanAnime,
}

impl SkinId {
    pub(crate) const ALL: [Self; 2] = [Self::Industrial, Self::CleanAnime];

    pub(crate) const fn localized_label(self, language: Language) -> &'static str {
        match self {
            Self::Industrial => language.pick("工业仪器", "Industrial instrument"),
            Self::CleanAnime => language.pick("日系风格", "Japanese style"),
        }
    }

    /// A compact label for fixed-width toolbar controls.  The full label is
    /// still used in settings, while this version keeps the header stable in
    /// both locales and at the compact window density.
    pub(crate) const fn localized_short_label(self, language: Language) -> &'static str {
        match self {
            Self::Industrial => language.pick("工业仪器", "Industrial"),
            Self::CleanAnime => language.pick("日系风格", "Japanese"),
        }
    }

    pub(crate) const fn localized_description(self, language: Language) -> &'static str {
        match self {
            Self::Industrial => language.pick(
                "灰阶专业仪表主题，适合长时间测量。",
                "A restrained grayscale instrument theme for long measurement sessions.",
            ),
            Self::CleanAnime => language.pick(
                "低饱和深色界面，铺陈日系角色壁纸。",
                "A low-saturation instrument interface with a Japanese character wallpaper.",
            ),
        }
    }
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) enum AutoCaptureMetric {
    #[default]
    Power,
    Current,
    Voltage,
}

impl AutoCaptureMetric {
    pub(crate) const ALL: [Self; 3] = [Self::Power, Self::Current, Self::Voltage];

    pub(crate) const fn localized_label(self, language: Language) -> &'static str {
        match self {
            Self::Power => language.pick("功率", "Power"),
            Self::Current => language.pick("电流", "Current"),
            Self::Voltage => language.pick("电压", "Voltage"),
        }
    }

    pub(crate) const fn milli_unit(self) -> &'static str {
        match self {
            Self::Power => "mW",
            Self::Current => "mA",
            Self::Voltage => "mV",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct AutoCaptureRule {
    pub(crate) enabled: bool,
    pub(crate) metric: AutoCaptureMetric,
    /// Threshold in the metric's milli-unit: mW, mA, or mV.
    pub(crate) threshold_milli: u32,
    pub(crate) sustain_ms: u32,
}

impl Default for AutoCaptureRule {
    fn default() -> Self {
        Self {
            enabled: false,
            metric: AutoCaptureMetric::Power,
            threshold_milli: 100,
            sustain_ms: 3_000,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) enum WorkspaceTab {
    #[default]
    Monitor,
    PdAnalysis,
}

/// A presentation-only filter for the monitor traces. The underlying samples,
/// recording statistics and exported files always remain untouched.
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) enum DisplayFilter {
    Raw,
    #[default]
    #[serde(alias = "Median3")]
    Median5,
}

/// Controls how the monitor plot maps each measurement channel to its
/// vertical coordinate.  Both modes keep the raw engineering values in the
/// cursor, cards, statistics and exported files; this only changes the axis
/// presentation.
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) enum ChartScaleMode {
    #[default]
    Actual,
    Relative,
}

impl ChartScaleMode {
    pub(crate) const ALL: [Self; 2] = [Self::Actual, Self::Relative];

    pub(crate) const fn localized_label(self, language: Language) -> &'static str {
        match self {
            Self::Actual => language.pick("实际刻度", "Engineering scale"),
            Self::Relative => language.pick("相对量程", "Relative range"),
        }
    }

    pub(crate) const fn localized_description(self, language: Language) -> &'static str {
        match self {
            Self::Actual => language.pick(
                "显示 V、A、W 的真实工程单位刻度。",
                "Show real engineering-unit ticks for V, A, and W.",
            ),
            Self::Relative => language.pick(
                "每条曲线独立显示 0–100%，游标仍显示真实值。",
                "Normalize each trace to 0–100%; the cursor still shows real values.",
            ),
        }
    }
}

impl DisplayFilter {
    pub(crate) const fn localized_label(self, language: Language) -> &'static str {
        match self {
            Self::Raw => language.pick("原始曲线", "Raw traces"),
            Self::Median5 => language.pick("五点中值降噪", "5-point median filter"),
        }
    }
}

/// Values that are safe to persist between launches. Device samples, serial
/// numbers and recording contents deliberately do not appear here.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub(crate) struct AppPreferences {
    pub(crate) language: Language,
    /// Presentation-only skin.  `serde(default)` keeps older preference
    /// files on the industrial theme when this field is absent.
    pub(crate) skin: SkinId,
    pub(crate) selected_rate: SampleRateOption,
    pub(crate) time_window: TimeWindow,
    pub(crate) plot_metrics: [PlotMetric; 3],
    pub(crate) recording_format: RecordingFormat,
    pub(crate) pd_auto_scroll: bool,
    pub(crate) pd_panel_visible: bool,
    pub(crate) pd_protocol_visible: bool,
    pub(crate) pd_trace_enabled: bool,
    pub(crate) usb_reset: bool,
    pub(crate) sleep_protection_enabled: bool,
    pub(crate) auto_pause_enabled: bool,
    pub(crate) auto_capture_metric: AutoCaptureMetric,
    pub(crate) auto_pause_threshold_mw: u32,
    pub(crate) auto_pause_delay_ms: u32,
    pub(crate) active_tab: WorkspaceTab,
    pub(crate) visible_series: [bool; 3],
    /// Visibility of the two cumulative traces on the monitor chart.
    /// Kept separate from the legacy three-item array so older preferences
    /// continue to deserialize without a migration prompt.
    pub(crate) visible_accumulated_series: [bool; 2],
    pub(crate) follow_latest: bool,
    pub(crate) display_filter: DisplayFilter,
    pub(crate) chart_scale_mode: ChartScaleMode,
}

impl Default for AppPreferences {
    fn default() -> Self {
        Self {
            language: Language::SimplifiedChinese,
            skin: SkinId::Industrial,
            selected_rate: SampleRateOption::Sps50,
            time_window: TimeWindow::Sec30,
            plot_metrics: [PlotMetric::Voltage, PlotMetric::Current, PlotMetric::Power],
            recording_format: RecordingFormat::Parquet,
            pd_auto_scroll: true,
            pd_panel_visible: true,
            pd_protocol_visible: true,
            pd_trace_enabled: false,
            usb_reset: false,
            sleep_protection_enabled: true,
            auto_pause_enabled: false,
            auto_capture_metric: AutoCaptureMetric::Power,
            auto_pause_threshold_mw: 100,
            auto_pause_delay_ms: 3_000,
            active_tab: WorkspaceTab::Monitor,
            visible_series: [true; 3],
            visible_accumulated_series: [true; 2],
            follow_latest: true,
            display_filter: DisplayFilter::Median5,
            chart_scale_mode: ChartScaleMode::Actual,
        }
    }
}
