use crate::connection::ConnectionPhase;
use serde::{Deserialize, Serialize};

pub(crate) const APP_TITLE: &str = "KM003C 工作台";
pub(crate) const APP_ID: &str = "com.weixun.km003cworkbench";
pub(crate) const APP_VERSION: &str = "0.1.0";
pub(crate) const APP_BUILD: &str = "1";

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) enum Language {
    #[default]
    SimplifiedChinese,
    English,
}

impl Language {
    pub(crate) const ALL: [Self; 2] = [Self::SimplifiedChinese, Self::English];

    pub(crate) const fn pick<'a>(self, zh_cn: &'a str, en: &'a str) -> &'a str {
        match self {
            Self::SimplifiedChinese => zh_cn,
            Self::English => en,
        }
    }

    pub(crate) const fn native_name(self) -> &'static str {
        match self {
            Self::SimplifiedChinese => "简体中文",
            Self::English => "English",
        }
    }

    pub(crate) const fn short_name(self) -> &'static str {
        match self {
            Self::SimplifiedChinese => "简中",
            Self::English => "EN",
        }
    }
}

pub(crate) const fn app_title(language: Language) -> &'static str {
    language.pick(APP_TITLE, "KM003C Workbench")
}

pub(crate) const fn connection_status(language: Language, phase: ConnectionPhase) -> &'static str {
    match phase {
        ConnectionPhase::Searching => language.pick("搜索设备", "Searching for device"),
        ConnectionPhase::Connecting => language.pick("连接中", "Connecting"),
        ConnectionPhase::Streaming => language.pick("设备采样中", "Sampling"),
        ConnectionPhase::NoDevice => language.pick("未发现设备", "Device not found"),
        ConnectionPhase::DeviceBusy => language.pick("设备被占用", "Device in use"),
        ConnectionPhase::ConnectionError => language.pick("连接错误", "Connection error"),
        ConnectionPhase::Disconnected => language.pick("已断开", "Disconnected"),
    }
}

/// Turn the stable portions of KM003C-lib's error text into an actionable
/// Chinese message while retaining the original error for the diagnostics log.
pub(crate) fn connection_error(language: Language, raw: &str) -> (ConnectionPhase, String) {
    let lower = raw.to_ascii_lowercase();
    if lower.contains("not found") || lower.contains("device not found") {
        return (
            ConnectionPhase::NoDevice,
            language
                .pick(
                    "未发现 KM003C。请插入数据线；系统会自动重试。",
                    "KM003C was not found. Connect a data-capable USB cable; the app will retry automatically.",
                )
                .to_string(),
        );
    }
    if lower.contains("busy") || lower.contains("in use") || lower.contains("resource") {
        return (
            ConnectionPhase::DeviceBusy,
            language
                .pick(
                    "设备被占用。请关闭其它 POWER-Z/USB 工具后再连接。",
                    "The device is in use. Close other POWER-Z or USB utilities, then reconnect.",
                )
                .to_string(),
        );
    }
    if lower.contains("adcqueue not enabled") || lower.contains("protocol") || lower.contains("invalid packet") {
        return (
            ConnectionPhase::ConnectionError,
            language
                .pick(
                    "设备协议或鉴权失败。请重新插拔设备，必要时在高级设置中手动 USB reset。",
                    "The device protocol or authentication failed. Reconnect the device; if needed, enable USB reset in Advanced settings.",
                )
                .to_string(),
        );
    }
    (
        ConnectionPhase::ConnectionError,
        format!("{}: {raw}", language.pick("连接错误", "Connection error")),
    )
}

pub(crate) fn original_error_context(language: Language, raw: &str) -> String {
    format!("{}: {raw}", language.pick("原始 USB 错误", "Original USB error"))
}
