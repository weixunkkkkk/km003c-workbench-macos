//! The USB session task: connects to the meter, streams AdcQueue samples,
//! PD events and the optional firmware trace to the UI, and serves the UI's
//! commands. It runs on the tokio runtime, independent of window painting.

use std::sync::Arc;
use std::time::{Duration, Instant};

use km003c_lib::{
    AdcQueueSample, DeviceConfig, DeviceState, GraphSampleRate, KM003C, LogMetadata, OfflineLog, PdTrace,
    packet::{Attribute, AttributeSet},
    pd::{PdEvent, PdStatus},
};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

/// How long streaming may go without AdcQueue samples before it is restarted.
/// The firmware stops streaming on its own when it is not polled for a while
/// (for example while the host was throttled or asleep) yet keeps answering
/// PD requests, so the session sees no error. At the slowest rate a sample is
/// due every 0.5 s.
const ADCQUEUE_STALL_TIMEOUT: Duration = Duration::from_secs(3);

/// Message from USB task to UI
#[derive(Debug, Clone)]
pub(crate) enum UsbMessage {
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
    /// No samples arrived since this instant; recording must retain the interruption.
    StreamingStalled(Instant),
    /// Error during streaming
    Error(String),
    /// Disconnected
    Disconnected,
}

/// Command from UI to USB task
#[derive(Debug, Clone)]
pub(crate) enum UsbCommand {
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

pub(crate) async fn usb_streaming_task(
    tx: mpsc::UnboundedSender<UsbMessage>,
    mut cmd_rx: mpsc::UnboundedReceiver<UsbCommand>,
) {
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
    let mut last_samples = Instant::now();
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

                    // Start with new rate. On failure the stall check below
                    // restarts the previous rate or reconnects.
                    if let Err(e) = start_streaming(&mut device, new_rate, tx).await {
                        error!("Failed to restart streaming: {}", e);
                        let _ = tx.send(UsbMessage::Error(format!("Restart failed: {}", e)));
                        continue;
                    }
                    current_rate = new_rate;
                    last_samples = Instant::now();
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
                last_samples = Instant::now();
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
                last_samples = Instant::now();
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
                    last_samples = Instant::now();
                    if tx.send(UsbMessage::Samples(queue_data.samples.clone())).is_err() {
                        warn!("UI closed, stopping");
                        break;
                    }
                } else if last_samples.elapsed() >= ADCQUEUE_STALL_TIMEOUT {
                    warn!(
                        "No AdcQueue samples for {:.1} s, restarting streaming",
                        last_samples.elapsed().as_secs_f32()
                    );
                    let _ = tx.send(UsbMessage::StreamingStalled(last_samples));
                    if let Err(error) = restart_streaming(&mut device, current_rate, tx).await {
                        // The firmware can reject StartGraph as well after a
                        // long stall. Ending the session hands it to the
                        // reconnect path, which authenticates again and
                        // resumes a recording that waits for this device.
                        warn!("Failed to restart stalled streaming, reconnecting: {error}");
                        break;
                    }
                    last_samples = Instant::now();
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
    // Release the interface before reporting: a quitting app waits for this
    // message and must not exit with the USB handle still open.
    drop(device);
    let _ = tx.send(UsbMessage::Disconnected);
}

pub(crate) fn streaming_attribute_mask(pd_trace_enabled: bool) -> AttributeSet {
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

async fn restart_streaming(
    device: &mut KM003C,
    rate: GraphSampleRate,
    tx: &mpsc::UnboundedSender<UsbMessage>,
) -> Result<(), km003c_lib::error::KMError> {
    device.stop_graph_mode().await?;
    let _ = tx.send(UsbMessage::StreamingStopped);
    start_streaming(device, rate, tx).await
}
