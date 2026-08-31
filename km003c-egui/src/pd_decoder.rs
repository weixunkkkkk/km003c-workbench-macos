use km003c_lib::pd::PdEvent;
use km003c_lib::uom::si::time::millisecond;
use km003c_lib::usbpd::protocol_layer::message::Payload;
use km003c_lib::usbpd::protocol_layer::message::data::source_capabilities::{
    Augmented, PowerDataObject, SourceCapabilities,
};
use km003c_lib::usbpd::protocol_layer::message::data::{self, Data};
use km003c_lib::usbpd::protocol_layer::message::extended::Extended;
use km003c_lib::usbpd::protocol_layer::message::header::{ControlMessageType, MessageType};
use km003c_lib::{DecodedPdEvent, DecodedPdMessage, PdChunkState, PdChunkStatus, PdDecodeFailure, PdSessionDecoder};

use crate::i18n::Language;

/// Category of a decoded PD entry, used for color-coding in the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PdCategory {
    Connect,
    Disconnect,
    SourceCaps,
    Request,
    Control,
    Extended,
    Contract,
    Error,
}

/// A single decoded PD log entry for display.
#[derive(Debug, Clone)]
pub struct DecodedPdEntry {
    pub timestamp_seconds: f64,
    pub category: PdCategory,
    pub summary: String,
    pub details: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PdContractKind {
    Fixed,
    Variable,
    Battery,
    Pps,
    EprFixed,
    EprPps,
    Avs,
    Unknown,
}

impl PdContractKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Fixed => "USB PD 固定档",
            Self::Variable => "USB PD 可变档",
            Self::Battery => "USB PD 电池档",
            Self::Pps => "USB PD PPS",
            Self::EprFixed => "USB PD EPR 固定档",
            Self::EprPps => "USB PD EPR PPS",
            Self::Avs => "USB PD EPR AVS",
            Self::Unknown => "USB PD",
        }
    }

    pub const fn localized_label(self, language: Language) -> &'static str {
        match self {
            Self::Fixed => language.pick("USB PD 固定档", "USB PD Fixed Supply"),
            Self::Variable => language.pick("USB PD 可变档", "USB PD Variable Supply"),
            Self::Battery => language.pick("USB PD 电池档", "USB PD Battery Supply"),
            Self::Pps => "USB PD PPS",
            Self::EprFixed => language.pick("USB PD EPR 固定档", "USB PD EPR Fixed Supply"),
            Self::EprPps => "USB PD EPR PPS",
            Self::Avs => "USB PD EPR AVS",
            Self::Unknown => "USB PD",
        }
    }

    const fn object_label(self) -> &'static str {
        match self {
            Self::Pps | Self::EprPps | Self::Avs => "APDO",
            _ => "PDO",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PdContract {
    pub kind: PdContractKind,
    pub object_position: u8,
    pub voltage_v: Option<f64>,
    pub current_a: Option<f64>,
    pub power_w: Option<f64>,
}

impl PdContract {
    pub fn summary(self) -> String {
        let object = format!("{}#{}", self.kind.object_label(), self.object_position);
        match (self.voltage_v, self.current_a, self.power_w) {
            (Some(voltage), Some(current), _) => format!(
                "{} · {object} · {}V / {}A",
                self.kind.label(),
                format_contract_number(voltage),
                format_contract_number(current),
            ),
            (Some(voltage), None, Some(power)) => format!(
                "{} · {object} · {}V / {}W",
                self.kind.label(),
                format_contract_number(voltage),
                format_contract_number(power),
            ),
            (_, _, Some(power)) => format!("{} · {object} · {}W", self.kind.label(), format_contract_number(power),),
            (_, Some(current), _) => format!(
                "{} · {object} · {}A",
                self.kind.label(),
                format_contract_number(current),
            ),
            _ => format!("{} · {object}", self.kind.label()),
        }
    }

    pub fn localized_summary(self, language: Language) -> String {
        let object = format!("{}#{}", self.kind.object_label(), self.object_position);
        let kind = self.kind.localized_label(language);
        match (self.voltage_v, self.current_a, self.power_w) {
            (Some(voltage), Some(current), _) => format!(
                "{kind} · {object} · {}V / {}A",
                format_contract_number(voltage),
                format_contract_number(current),
            ),
            (Some(voltage), None, Some(power)) => format!(
                "{kind} · {object} · {}V / {}W",
                format_contract_number(voltage),
                format_contract_number(power),
            ),
            (_, _, Some(power)) => format!("{kind} · {object} · {}W", format_contract_number(power)),
            (_, Some(current), _) => format!("{kind} · {object} · {}A", format_contract_number(current)),
            _ => format!("{kind} · {object}"),
        }
    }
}

fn format_contract_number(value: f64) -> String {
    if (value - value.round()).abs() < 0.005 {
        format!("{value:.0}")
    } else if (value * 10.0 - (value * 10.0).round()).abs() < 0.005 {
        format!("{value:.1}")
    } else {
        format!("{value:.2}")
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PowerProtocolState {
    Unavailable,
    Disconnected,
    Waiting,
    TraditionalUnconfirmed,
    PdDetected,
    Negotiating(PdContract),
    Confirmed(PdContract),
}

impl PowerProtocolState {
    pub const fn localized_status_label(self, language: Language) -> &'static str {
        match self {
            Self::Unavailable => language.pick("无协议数据", "No protocol data"),
            Self::Disconnected => language.pick("Type-C 未连接", "Type-C disconnected"),
            Self::Waiting => language.pick("等待协商", "Waiting for negotiation"),
            Self::TraditionalUnconfirmed => language.pick("协议未确认", "Protocol unconfirmed"),
            Self::PdDetected => language.pick("检测到 USB PD", "USB PD detected"),
            Self::Negotiating(_) => language.pick("PD 协商中", "PD negotiation in progress"),
            Self::Confirmed(_) => language.pick("已确认", "Confirmed"),
        }
    }

    pub const fn contract(self) -> Option<PdContract> {
        match self {
            Self::Negotiating(contract) | Self::Confirmed(contract) => Some(contract),
            _ => None,
        }
    }
}

/// UI formatter backed by the shared stateful decoder in `km003c-lib`.
pub struct PdDecoder {
    session: PdSessionDecoder,
    protocol_state: PowerProtocolState,
    pending_contract: Option<PdContract>,
    active_contract: Option<PdContract>,
    pending_accepted: bool,
}

impl PdDecoder {
    pub fn new() -> Self {
        Self {
            session: PdSessionDecoder::new(),
            protocol_state: PowerProtocolState::Disconnected,
            pending_contract: None,
            active_contract: None,
            pending_accepted: false,
        }
    }

    pub fn reset(&mut self) {
        self.session.reset();
        self.protocol_state = PowerProtocolState::Disconnected;
        self.pending_contract = None;
        self.active_contract = None;
        self.pending_accepted = false;
    }

    #[cfg(test)]
    pub const fn protocol_state(&self) -> PowerProtocolState {
        self.protocol_state
    }

    pub fn display_state(&self, connected: Option<bool>) -> PowerProtocolState {
        match self.protocol_state {
            PowerProtocolState::PdDetected | PowerProtocolState::Negotiating(_) | PowerProtocolState::Confirmed(_) => {
                self.protocol_state
            }
            PowerProtocolState::Disconnected | PowerProtocolState::Waiting => match connected {
                Some(true) => PowerProtocolState::TraditionalUnconfirmed,
                None => PowerProtocolState::Waiting,
                Some(false) => PowerProtocolState::Disconnected,
            },
            state => state,
        }
    }

    pub fn decode_event(&mut self, event: &PdEvent) -> Vec<DecodedPdEntry> {
        let decoded = self.session.decode_event(event);
        let previous_state = self.protocol_state;
        self.observe_decoded(&decoded);
        let mut entries = vec![match decoded {
            DecodedPdEvent::Connect { timestamp } => DecodedPdEntry {
                timestamp_seconds: timestamp.get::<millisecond>() / 1000.0,
                category: PdCategory::Connect,
                summary: format!("[{:.3}s] ** CONNECT **", timestamp.get::<millisecond>() / 1000.0),
                details: vec![],
            },
            DecodedPdEvent::Disconnect { timestamp } => DecodedPdEntry {
                timestamp_seconds: timestamp.get::<millisecond>() / 1000.0,
                category: PdCategory::Disconnect,
                summary: format!("[{:.3}s] ** DISCONNECT **", timestamp.get::<millisecond>() / 1000.0),
                details: vec![],
            },
            DecodedPdEvent::Message(message) => self.format_message(&message),
            DecodedPdEvent::Chunk(status) => format_chunk_status(status),
            DecodedPdEvent::Error(failure) => format_failure(failure),
        }];
        if let PowerProtocolState::Confirmed(contract) = self.protocol_state
            && previous_state != self.protocol_state
        {
            entries.push(DecodedPdEntry {
                timestamp_seconds: entries[0].timestamp_seconds,
                category: PdCategory::Contract,
                summary: format!("当前协议已确认：{}", contract.summary()),
                details: vec!["Request → Accept → PS_RDY 完整协商链路".to_string()],
            });
        }
        entries
    }

    fn observe_decoded(&mut self, decoded: &DecodedPdEvent) {
        match decoded {
            DecodedPdEvent::Connect { .. } => {
                self.protocol_state = PowerProtocolState::Waiting;
                self.pending_contract = None;
                self.active_contract = None;
                self.pending_accepted = false;
            }
            DecodedPdEvent::Disconnect { .. } => self.reset(),
            DecodedPdEvent::Message(message) => self.observe_message(message),
            DecodedPdEvent::Chunk(_) | DecodedPdEvent::Error(_) => {}
        }
    }

    fn observe_message(&mut self, decoded: &DecodedPdMessage) {
        // SOP' and SOP'' describe cable/e-marker traffic, not the active
        // source-to-sink power contract shown on the monitor page.
        if decoded.sop != 0 {
            return;
        }
        match &decoded.message.payload {
            Some(Payload::Data(Data::SourceCapabilities(_)))
            | Some(Payload::Extended(Extended::EprSourceCapabilities(_))) => {
                self.pending_contract = None;
                self.pending_accepted = false;
                self.protocol_state = self
                    .active_contract
                    .map_or(PowerProtocolState::PdDetected, PowerProtocolState::Confirmed);
            }
            Some(Payload::Data(Data::Request(request))) => {
                self.pending_contract = contract_from_request(request, self.session.source_capabilities());
                self.pending_accepted = false;
                self.protocol_state = self
                    .pending_contract
                    .map_or(PowerProtocolState::PdDetected, PowerProtocolState::Negotiating);
            }
            _ => match decoded.message.header.message_type() {
                MessageType::Control(ControlMessageType::Accept) if self.pending_contract.is_some() => {
                    self.pending_accepted = true;
                }
                MessageType::Control(ControlMessageType::PsRdy)
                    if self.pending_accepted && self.pending_contract.is_some() =>
                {
                    let contract = self.pending_contract.take().unwrap();
                    self.active_contract = Some(contract);
                    self.pending_accepted = false;
                    self.protocol_state = PowerProtocolState::Confirmed(contract);
                }
                MessageType::Control(ControlMessageType::Reject | ControlMessageType::Wait) => {
                    self.pending_contract = None;
                    self.pending_accepted = false;
                    self.protocol_state = self
                        .active_contract
                        .map_or(PowerProtocolState::PdDetected, PowerProtocolState::Confirmed);
                }
                MessageType::Control(ControlMessageType::SoftReset) => {
                    self.pending_contract = None;
                    self.active_contract = None;
                    self.pending_accepted = false;
                    self.protocol_state = PowerProtocolState::PdDetected;
                }
                _ => {}
            },
        }
    }

    fn format_message(&self, decoded: &DecodedPdMessage) -> DecodedPdEntry {
        let message = &decoded.message;
        let message_type = message.header.message_type();
        let timestamp_seconds = decoded.timestamp.get::<millisecond>() / 1000.0;
        let summary = format!(
            "[{:.3}s] SOP{}: {:?} (ID={}, ROLE={:?}/{:?})",
            decoded.timestamp.get::<millisecond>() / 1000.0,
            decoded.sop,
            message_type,
            message.header.message_id(),
            message.header.port_power_role(),
            message.header.port_data_role(),
        );

        match &message.payload {
            Some(Payload::Data(Data::SourceCapabilities(capabilities))) => DecodedPdEntry {
                timestamp_seconds,
                category: PdCategory::SourceCaps,
                summary,
                details: format_capabilities(capabilities.pdos(), "SPR Source Capabilities"),
            },
            Some(Payload::Data(Data::Request(request))) => DecodedPdEntry {
                timestamp_seconds,
                category: PdCategory::Request,
                summary,
                details: format_request(request, self.session.source_capabilities()),
            },
            Some(Payload::Data(Data::EprMode(mode))) => DecodedPdEntry {
                timestamp_seconds,
                category: PdCategory::Extended,
                summary,
                details: vec![format!("EPR Mode: {mode:?}")],
            },
            Some(Payload::Data(Data::Unknown)) => DecodedPdEntry {
                timestamp_seconds,
                category: PdCategory::Control,
                summary,
                details: vec!["Unknown Data Message".to_string()],
            },
            Some(Payload::Data(data)) => DecodedPdEntry {
                timestamp_seconds,
                category: PdCategory::Control,
                summary,
                details: vec![format!("Data: {data:?}")],
            },
            Some(Payload::Extended(Extended::EprSourceCapabilities(pdos))) => DecodedPdEntry {
                timestamp_seconds,
                category: PdCategory::Extended,
                summary,
                details: format_capabilities(pdos.as_slice(), "EPR Source Capabilities"),
            },
            Some(Payload::Extended(Extended::ExtendedControl(control))) => DecodedPdEntry {
                timestamp_seconds,
                category: PdCategory::Extended,
                summary,
                details: vec![format!(
                    "Extended Control: {:?} (data=0x{:02X})",
                    control.message_type(),
                    control.data()
                )],
            },
            Some(Payload::Extended(extended)) => DecodedPdEntry {
                timestamp_seconds,
                category: PdCategory::Extended,
                summary,
                details: vec![format!("Extended: {extended:?}")],
            },
            None => DecodedPdEntry {
                timestamp_seconds,
                category: PdCategory::Control,
                summary,
                details: vec![],
            },
        }
    }
}

fn contract_from_request(
    request: &data::request::PowerSource,
    source_caps: Option<&SourceCapabilities>,
) -> Option<PdContract> {
    use data::request::{Avs as RdoAvs, FixedVariableSupply as RdoFixed, PowerSource, RawDataObject};

    let selected_pdo = |position: u8| {
        source_caps.and_then(|capabilities| capabilities.pdos().get(position.saturating_sub(1) as usize))
    };
    match request {
        PowerSource::FixedVariableSupply(request) => {
            let position = request.object_position();
            let (kind, voltage_v, power_w) = match selected_pdo(position) {
                Some(PowerDataObject::FixedSupply(fixed)) => {
                    (PdContractKind::Fixed, Some(f64::from(fixed.raw_voltage()) * 0.05), None)
                }
                Some(PowerDataObject::VariableSupply(variable)) => (
                    PdContractKind::Variable,
                    Some(f64::from(variable.raw_max_voltage()) * 0.05),
                    None,
                ),
                Some(PowerDataObject::Battery(battery)) => (
                    PdContractKind::Battery,
                    None,
                    Some(f64::from(battery.raw_max_power()) * 0.25),
                ),
                _ => (PdContractKind::Fixed, None, None),
            };
            Some(PdContract {
                kind,
                object_position: position,
                voltage_v,
                current_a: Some(f64::from(request.raw_operating_current()) * 0.01),
                power_w,
            })
        }
        PowerSource::Battery(request) => Some(PdContract {
            kind: PdContractKind::Battery,
            object_position: request.object_position(),
            voltage_v: None,
            current_a: None,
            power_w: Some(f64::from(request.raw_operating_power()) * 0.25),
        }),
        PowerSource::Pps(request) => Some(PdContract {
            kind: PdContractKind::Pps,
            object_position: request.object_position(),
            voltage_v: Some(f64::from(request.raw_output_voltage()) * 0.02),
            current_a: Some(f64::from(request.raw_operating_current()) * 0.05),
            power_w: None,
        }),
        PowerSource::Avs(request) => Some(PdContract {
            kind: PdContractKind::Avs,
            object_position: request.object_position(),
            voltage_v: Some(f64::from(request.raw_output_voltage()) * 0.025),
            current_a: Some(f64::from(request.raw_operating_current()) * 0.05),
            power_w: None,
        }),
        PowerSource::EprRequest { rdo, pdo } => {
            let position = RawDataObject(*rdo).object_position();
            let (kind, voltage_v, current_a, power_w) = match pdo {
                PowerDataObject::FixedSupply(fixed) => {
                    let request = RdoFixed(*rdo);
                    (
                        PdContractKind::EprFixed,
                        Some(f64::from(fixed.raw_voltage()) * 0.05),
                        Some(f64::from(request.raw_operating_current()) * 0.01),
                        None,
                    )
                }
                PowerDataObject::Augmented(Augmented::Spr(_)) => {
                    let request = RdoAvs(*rdo);
                    (
                        PdContractKind::EprPps,
                        Some(f64::from(request.raw_output_voltage()) * 0.025),
                        Some(f64::from(request.raw_operating_current()) * 0.05),
                        None,
                    )
                }
                PowerDataObject::Augmented(Augmented::Epr(avs)) => {
                    let request = RdoAvs(*rdo);
                    (
                        PdContractKind::Avs,
                        Some(f64::from(request.raw_output_voltage()) * 0.025),
                        Some(f64::from(request.raw_operating_current()) * 0.05),
                        Some(f64::from(avs.raw_pd_power())),
                    )
                }
                PowerDataObject::Battery(battery) => (
                    PdContractKind::Battery,
                    None,
                    None,
                    Some(f64::from(battery.raw_max_power()) * 0.25),
                ),
                PowerDataObject::VariableSupply(variable) => (
                    PdContractKind::Variable,
                    Some(f64::from(variable.raw_max_voltage()) * 0.05),
                    None,
                    None,
                ),
                _ => (PdContractKind::Unknown, None, None, None),
            };
            Some(PdContract {
                kind,
                object_position: position,
                voltage_v,
                current_a,
                power_w,
            })
        }
        PowerSource::Unknown(raw) => {
            let position = raw.object_position();
            let request = RdoFixed(raw.0);
            Some(PdContract {
                kind: PdContractKind::Unknown,
                object_position: position,
                voltage_v: None,
                current_a: Some(f64::from(request.raw_operating_current()) * 0.01),
                power_w: None,
            })
        }
    }
}

fn format_chunk_status(status: PdChunkStatus) -> DecodedPdEntry {
    let timestamp = status.timestamp.get::<millisecond>() / 1000.0;
    let summary = match status.state {
        PdChunkState::Request { chunk_number } => format!(
            "[{timestamp:.3}s] SOP{}: Chunk Request (chunk={chunk_number}, type={:?})",
            status.sop, status.message_type
        ),
        PdChunkState::Pending {
            received_chunk,
            next_chunk,
        } => format!(
            "[{timestamp:.3}s] SOP{}: {:?} chunk {received_chunk} received, waiting for chunk {next_chunk}",
            status.sop, status.message_type
        ),
        PdChunkState::Requested { chunk_number } => format!(
            "[{timestamp:.3}s] SOP{}: {:?} chunk {chunk_number} requested",
            status.sop, status.message_type
        ),
        PdChunkState::Unsupported {
            chunk_number,
            data_size,
        } => format!(
            "[{timestamp:.3}s] SOP{}: Chunked {:?} (chunk {chunk_number}, {data_size} bytes) - not assembled",
            status.sop, status.message_type
        ),
    };

    DecodedPdEntry {
        timestamp_seconds: timestamp,
        category: PdCategory::Extended,
        summary,
        details: vec![],
    }
}

fn format_failure(failure: PdDecodeFailure) -> DecodedPdEntry {
    let timestamp_seconds = failure.timestamp.get::<millisecond>() / 1000.0;
    DecodedPdEntry {
        timestamp_seconds,
        category: PdCategory::Error,
        summary: format!(
            "[{:.3}s] SOP{}: Parse error: {}",
            failure.timestamp.get::<millisecond>() / 1000.0,
            failure.sop,
            failure.error
        ),
        details: vec![format!("Hex: {:02X?}", failure.wire_data)],
    }
}

fn format_request(request: &data::request::PowerSource, source_caps: Option<&SourceCapabilities>) -> Vec<String> {
    use data::request::PowerSource;

    match request {
        PowerSource::FixedVariableSupply(request) => {
            let current = f64::from(request.raw_operating_current()) * 0.01;
            let max_current = f64::from(request.raw_max_operating_current()) * 0.01;
            let position = request.object_position();
            let pdo = source_caps
                .and_then(|capabilities| capabilities.pdos().get(position.saturating_sub(1) as usize))
                .map(format_pdo);

            if let Some(pdo) = pdo {
                vec![format!("RDO: PDO#{position} ({pdo}) @ {current:.1}A")]
            } else {
                vec![format!("RDO: PDO#{position} @ {current:.1}A (Max {max_current:.1}A)")]
            }
        }
        PowerSource::Battery(request) => vec![format!(
            "RDO: Requesting Battery PDO#{} @ {:.2}W",
            request.object_position(),
            f64::from(request.raw_operating_power()) * 0.25
        )],
        PowerSource::Pps(request) => vec![format!(
            "RDO: Requesting PPS PDO#{} @ {:.2}V / {:.2}A",
            request.object_position(),
            f64::from(request.raw_output_voltage()) * 0.02,
            f64::from(request.raw_operating_current()) * 0.05
        )],
        PowerSource::Avs(request) => vec![format!(
            "RDO: Requesting AVS PDO#{} @ {:.2}V / {:.2}A",
            request.object_position(),
            f64::from(request.raw_output_voltage()) * 0.025,
            f64::from(request.raw_operating_current()) * 0.05
        )],
        PowerSource::EprRequest { rdo, pdo } => {
            use data::request::{Avs as RdoAvs, FixedVariableSupply as RdoFixed, RawDataObject};

            let position = RawDataObject(*rdo).object_position();
            match pdo {
                PowerDataObject::FixedSupply(fixed) => {
                    let request = RdoFixed(*rdo);
                    vec![format!(
                        "RDO: EPR Fixed PDO#{position} ({:.1}V) @ {:.2}A (Max {:.2}A)",
                        f64::from(fixed.raw_voltage()) * 0.05,
                        f64::from(request.raw_operating_current()) * 0.01,
                        f64::from(request.raw_max_operating_current()) * 0.01
                    )]
                }
                PowerDataObject::Augmented(Augmented::Spr(pps)) => {
                    let request = RdoAvs(*rdo);
                    vec![format!(
                        "RDO: EPR PPS PDO#{position} ({:.1}-{:.1}V) @ {:.2}V / {:.2}A",
                        f64::from(pps.raw_min_voltage()) * 0.1,
                        f64::from(pps.raw_max_voltage()) * 0.1,
                        f64::from(request.raw_output_voltage()) * 0.025,
                        f64::from(request.raw_operating_current()) * 0.05
                    )]
                }
                PowerDataObject::Augmented(Augmented::Epr(avs)) => {
                    let request = RdoAvs(*rdo);
                    vec![format!(
                        "RDO: EPR AVS PDO#{position} ({:.1}-{:.1}V @ {:.0}W) @ {:.2}V / {:.2}A",
                        f64::from(avs.raw_min_voltage()) * 0.1,
                        f64::from(avs.raw_max_voltage()) * 0.1,
                        f64::from(avs.raw_pd_power()),
                        f64::from(request.raw_output_voltage()) * 0.025,
                        f64::from(request.raw_operating_current()) * 0.05
                    )]
                }
                PowerDataObject::Augmented(_) => {
                    vec![format!("RDO: EPR Augmented PDO#{position} (Raw=0x{rdo:08X})")]
                }
                _ => vec![format!("RDO: EPR PDO#{position} (Raw=0x{rdo:08X}, PDO={pdo:?})")],
            }
        }
        PowerSource::Unknown(raw) => {
            let position = raw.object_position();
            if let Some(pdo) =
                source_caps.and_then(|capabilities| capabilities.pdos().get(position.saturating_sub(1) as usize))
            {
                let request = data::request::FixedVariableSupply(raw.0);
                vec![format!(
                    "RDO: Requesting PDO#{position} ({}) @ {:.1}A",
                    format_pdo(pdo),
                    f64::from(request.raw_operating_current()) * 0.01
                )]
            } else {
                vec![format!("RDO: Requesting PDO#{position} (Raw=0x{:08X})", raw.0)]
            }
        }
    }
}

fn format_pdo(pdo: &PowerDataObject) -> String {
    match pdo {
        PowerDataObject::FixedSupply(fixed) => {
            let voltage = f64::from(fixed.raw_voltage()) * 0.05;
            let current = f64::from(fixed.raw_max_current()) * 0.01;
            let mut flags = Vec::new();
            if fixed.dual_role_power() {
                flags.push("DRP");
            }
            if fixed.usb_communications_capable() {
                flags.push("USB");
            }
            if fixed.dual_role_data() {
                flags.push("DRD");
            }
            if fixed.unconstrained_power() {
                flags.push("UP");
            }
            if fixed.epr_mode_capable() {
                flags.push("EPR");
            }
            let flags = if flags.is_empty() {
                String::new()
            } else {
                format!(" [{}]", flags.join(","))
            };
            format!("Fixed {voltage:.0}V @ {current:.1}A ({:.0}W){flags}", voltage * current)
        }
        PowerDataObject::Battery(battery) => format!(
            "Battery {:.0}-{:.0}V @ {:.0}W",
            f64::from(battery.raw_min_voltage()) * 0.05,
            f64::from(battery.raw_max_voltage()) * 0.05,
            f64::from(battery.raw_max_power()) * 0.25
        ),
        PowerDataObject::VariableSupply(variable) => format!(
            "Variable {:.0}-{:.0}V @ {:.1}A",
            f64::from(variable.raw_min_voltage()) * 0.05,
            f64::from(variable.raw_max_voltage()) * 0.05,
            f64::from(variable.raw_max_current()) * 0.01
        ),
        PowerDataObject::Augmented(Augmented::Spr(pps)) => {
            let min_voltage = f64::from(pps.raw_min_voltage()) * 0.1;
            let max_voltage = f64::from(pps.raw_max_voltage()) * 0.1;
            let current = f64::from(pps.raw_max_current()) * 0.05;
            let limited = if pps.pps_power_limited() { " (limited)" } else { "" };
            format!(
                "PPS {min_voltage:.1}-{max_voltage:.1}V @ {current:.1}A ({:.0}W){limited}",
                max_voltage * current
            )
        }
        PowerDataObject::Augmented(Augmented::Epr(avs)) => format!(
            "EPR AVS {:.0}-{:.0}V @ {:.0}W",
            f64::from(avs.raw_min_voltage()) * 0.1,
            f64::from(avs.raw_max_voltage()) * 0.1,
            f64::from(avs.raw_pd_power())
        ),
        PowerDataObject::Augmented(Augmented::Unknown(raw)) => format!("Augmented(0x{raw:08X})"),
        PowerDataObject::Unknown(raw) => format!("Unknown(0x{:08X})", raw.0),
    }
}

fn format_capabilities(capabilities: &[PowerDataObject], title: &str) -> Vec<String> {
    let mut lines = vec![format!("[{title}]")];
    for (index, pdo) in capabilities.iter().enumerate() {
        if matches!(pdo, PowerDataObject::FixedSupply(fixed) if fixed.0 == 0) {
            lines.push(format!("PDO[{}]: --- (separator) ---", index + 1));
        } else {
            lines.push(format!("PDO[{}]: {}", index + 1, format_pdo(pdo)));
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use km003c_lib::pd::PdEventData;

    fn wire_event(timestamp_ms: f64, wire_hex: &str) -> PdEvent {
        PdEvent {
            timestamp: km003c_lib::uom::si::f64::Time::new::<millisecond>(timestamp_ms),
            data: PdEventData::PdMessage {
                sop: 0,
                wire_data: hex::decode(wire_hex).unwrap(),
            },
        }
    }

    #[test]
    fn confirms_a_contract_only_after_request_accept_and_ps_rdy() {
        // The capabilities and request are from a KM003C capture. The capture
        // ended in GoodCRC, so a valid PS_RDY control header is appended here
        // to exercise the required complete confirmation chain.
        let events = [
            wire_event(1.0, "a1632c9101082cd102002cc103002cb10400454106003c21dcc0"),
            wire_event(2.0, "8210dc700323"),
            wire_event(3.0, "a305"),
            wire_event(4.0, "4604"),
        ];
        let mut decoder = PdDecoder::new();

        decoder.decode_event(&events[0]);
        assert_eq!(decoder.protocol_state(), PowerProtocolState::PdDetected);

        decoder.decode_event(&events[1]);
        let PowerProtocolState::Negotiating(contract) = decoder.protocol_state() else {
            panic!("request must create a pending contract");
        };
        assert_eq!(contract.kind, PdContractKind::Fixed);
        assert_eq!(contract.object_position, 2);
        assert_eq!(contract.voltage_v, Some(9.0));

        decoder.decode_event(&events[2]);
        assert!(matches!(decoder.protocol_state(), PowerProtocolState::Negotiating(_)));

        let entries = decoder.decode_event(&events[3]);
        let PowerProtocolState::Confirmed(contract) = decoder.protocol_state() else {
            panic!("PS_RDY after Accept must confirm the contract");
        };
        assert_eq!(contract.summary(), "USB PD 固定档 · PDO#2 · 9V / 2.2A");
        assert!(entries.iter().any(|entry| entry.category == PdCategory::Contract));
        assert!(matches!(
            decoder.display_state(Some(false)),
            PowerProtocolState::Confirmed(_)
        ));

        decoder.decode_event(&PdEvent {
            timestamp: km003c_lib::uom::si::f64::Time::new::<millisecond>(5.0),
            data: PdEventData::Disconnect(()),
        });
        assert_eq!(decoder.display_state(Some(false)), PowerProtocolState::Disconnected);
    }

    #[test]
    fn source_capabilities_or_rejected_request_never_claim_an_active_contract() {
        let source_caps = wire_event(1.0, "a1632c9101082cd102002cc103002cb10400454106003c21dcc0");
        let request = wire_event(2.0, "8210dc700323");
        let reject = wire_event(3.0, "a405");
        let mut decoder = PdDecoder::new();

        decoder.decode_event(&source_caps);
        assert_eq!(decoder.protocol_state(), PowerProtocolState::PdDetected);
        decoder.decode_event(&request);
        assert!(matches!(decoder.protocol_state(), PowerProtocolState::Negotiating(_)));
        decoder.decode_event(&reject);
        assert_eq!(decoder.protocol_state(), PowerProtocolState::PdDetected);
    }

    #[test]
    fn typed_requests_preserve_pps_and_avs_setpoints() {
        let pps = data::request::Pps(0)
            .with_object_position(3)
            .with_raw_output_voltage(442)
            .with_raw_operating_current(50);
        let pps_contract = contract_from_request(&data::request::PowerSource::Pps(pps), None).unwrap();
        assert_eq!(pps_contract.kind, PdContractKind::Pps);
        assert_eq!(pps_contract.voltage_v, Some(8.84));
        assert_eq!(pps_contract.current_a, Some(2.5));
        assert_eq!(pps_contract.summary(), "USB PD PPS · APDO#3 · 8.84V / 2.5A");

        let avs = data::request::Avs(0)
            .with_object_position(8)
            .with_raw_output_voltage(1_120)
            .with_raw_operating_current(60);
        let avs_contract = contract_from_request(&data::request::PowerSource::Avs(avs), None).unwrap();
        assert_eq!(avs_contract.kind, PdContractKind::Avs);
        assert_eq!(avs_contract.voltage_v, Some(28.0));
        assert_eq!(avs_contract.current_a, Some(3.0));
        assert_eq!(avs_contract.summary(), "USB PD EPR AVS · APDO#8 · 28V / 3A");
    }

    #[test]
    fn attached_without_pd_messages_is_explicitly_unconfirmed() {
        let decoder = PdDecoder::new();
        assert_eq!(
            decoder.display_state(Some(true)),
            PowerProtocolState::TraditionalUnconfirmed
        );
        assert_eq!(decoder.display_state(Some(false)), PowerProtocolState::Disconnected);
    }
}
