/// The small, explicit connection state machine shown in the status rail.
///
/// `DeviceNotFound` is intentionally represented by `NoDevice`, while busy
/// and protocol failures have their own terminal states. This lets the UI
/// retry only the safe case without hiding actionable USB errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectionPhase {
    Searching,
    Connecting,
    Streaming,
    NoDevice,
    DeviceBusy,
    ConnectionError,
    Disconnected,
}

impl ConnectionPhase {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Searching => "搜索设备",
            Self::Connecting => "连接中",
            Self::Streaming => "设备采样中",
            Self::NoDevice => "未发现设备",
            Self::DeviceBusy => "设备被占用",
            Self::ConnectionError => "连接错误",
            Self::Disconnected => "已断开",
        }
    }

    #[allow(dead_code)]
    pub(crate) const fn is_terminal_failure(self) -> bool {
        matches!(self, Self::DeviceBusy | Self::ConnectionError)
    }
}
