use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use chrono_tz::Asia::Shanghai;
use polars::prelude::{CsvReader, DataFrame, DataType, KeyValueMetadata, ParquetReader, ParquetWriter, SerReader};
use serde::{Deserialize, Serialize};

use crate::recording::{RecordingFormat, RecordingMetadata, RecordingSummary};

pub(crate) const SESSION_SCHEMA_VERSION: u32 = 1;
pub(crate) const DISPLAY_TIMEZONE: &str = "Asia/Shanghai";
pub(crate) const PARQUET_SESSION_METADATA_KEY: &str = "km003c.session_metadata_v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SessionState {
    Recording,
    Paused,
    WaitingForReconnect,
    Finalizing,
    Saved,
    Interrupted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IntervalReason {
    ManualPause,
    AutomaticPause,
    UsbDisconnected,
    ApplicationRestart,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RecordingTimeInterval {
    pub(crate) reason: IntervalReason,
    pub(crate) started_at_utc: DateTime<Utc>,
    pub(crate) ended_at_utc: Option<DateTime<Utc>>,
}

impl RecordingTimeInterval {
    pub(crate) fn duration_ms(&self, now: DateTime<Utc>) -> u64 {
        self.ended_at_utc
            .unwrap_or(now)
            .signed_duration_since(self.started_at_utc)
            .num_milliseconds()
            .max(0) as u64
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RecordingTimestamps {
    pub(crate) started_at_utc: DateTime<Utc>,
    pub(crate) ended_at_utc: Option<DateTime<Utc>>,
    pub(crate) saved_at_utc: Option<DateTime<Utc>>,
    pub(crate) display_timezone: String,
    pub(crate) started_at_beijing: String,
    pub(crate) ended_at_beijing: Option<String>,
    pub(crate) saved_at_beijing: Option<String>,
}

impl RecordingTimestamps {
    fn new(started_at_utc: DateTime<Utc>) -> Self {
        Self {
            started_at_utc,
            ended_at_utc: None,
            saved_at_utc: None,
            display_timezone: DISPLAY_TIMEZONE.to_string(),
            started_at_beijing: format_beijing(started_at_utc),
            ended_at_beijing: None,
            saved_at_beijing: None,
        }
    }

    pub(crate) fn set_ended(&mut self, ended_at_utc: DateTime<Utc>) {
        self.ended_at_utc = Some(ended_at_utc);
        self.ended_at_beijing = Some(format_beijing(ended_at_utc));
    }

    pub(crate) fn set_saved(&mut self, saved_at_utc: DateTime<Utc>) {
        self.saved_at_utc = Some(saved_at_utc);
        self.saved_at_beijing = Some(format_beijing(saved_at_utc));
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct RecordingSessionMetadataV1 {
    pub(crate) schema_version: u32,
    pub(crate) session_id: String,
    pub(crate) timestamps: RecordingTimestamps,
    pub(crate) device: RecordingMetadata,
    pub(crate) sample_rate_hz: u32,
    pub(crate) rows: u64,
    pub(crate) effective_duration_us: u64,
    pub(crate) wall_clock_duration_ms: u64,
    pub(crate) paused_duration_ms: u64,
    pub(crate) disconnected_duration_ms: u64,
    pub(crate) missing_samples: u64,
    pub(crate) interpolated_duration_us: u64,
    pub(crate) discarded_sequence_samples: u64,
    pub(crate) net_charge_uah: f64,
    pub(crate) net_energy_uwh: f64,
    pub(crate) cumulative_capacity_uah: f64,
    pub(crate) cumulative_energy_uwh: f64,
    pub(crate) completeness_percent: f64,
    pub(crate) pause_intervals: Vec<RecordingTimeInterval>,
    pub(crate) disconnect_intervals: Vec<RecordingTimeInterval>,
}

impl Default for RecordingSessionMetadataV1 {
    fn default() -> Self {
        let now = Utc::now();
        Self {
            schema_version: SESSION_SCHEMA_VERSION,
            session_id: create_session_id(now),
            timestamps: RecordingTimestamps::new(now),
            device: RecordingMetadata::default(),
            sample_rate_hz: 50,
            rows: 0,
            effective_duration_us: 0,
            wall_clock_duration_ms: 0,
            paused_duration_ms: 0,
            disconnected_duration_ms: 0,
            missing_samples: 0,
            interpolated_duration_us: 0,
            discarded_sequence_samples: 0,
            net_charge_uah: 0.0,
            net_energy_uwh: 0.0,
            cumulative_capacity_uah: 0.0,
            cumulative_energy_uwh: 0.0,
            completeness_percent: 100.0,
            pause_intervals: Vec::new(),
            disconnect_intervals: Vec::new(),
        }
    }
}

impl RecordingSessionMetadataV1 {
    pub(crate) fn new(started_at_utc: DateTime<Utc>, device: RecordingMetadata, sample_rate_hz: u32) -> Self {
        Self {
            session_id: create_session_id(started_at_utc),
            timestamps: RecordingTimestamps::new(started_at_utc),
            device,
            sample_rate_hz,
            ..Self::default()
        }
    }

    pub(crate) fn refresh_durations(&mut self, now: DateTime<Utc>) {
        let end = self.timestamps.ended_at_utc.unwrap_or(now);
        self.wall_clock_duration_ms = end
            .signed_duration_since(self.timestamps.started_at_utc)
            .num_milliseconds()
            .max(0) as u64;
        self.paused_duration_ms = self
            .pause_intervals
            .iter()
            .map(|interval| interval.duration_ms(now))
            .sum();
        self.disconnected_duration_ms = self
            .disconnect_intervals
            .iter()
            .map(|interval| interval.duration_ms(now))
            .sum();
    }

    pub(crate) fn update_from_summary(&mut self, summary: &RecordingSummary) {
        self.rows = summary.rows;
        self.effective_duration_us = summary.elapsed_us;
        self.missing_samples = summary.missing_samples;
        self.interpolated_duration_us = summary.interpolated_duration_us;
        self.discarded_sequence_samples = summary.discarded_sequence_samples;
        self.net_charge_uah = summary.charge_uah;
        self.net_energy_uwh = summary.energy_uwh;
        self.cumulative_capacity_uah = summary.charge_throughput_uah;
        self.cumulative_energy_uwh = summary.energy_throughput_uwh;
        self.completeness_percent = summary.completeness_percent();
    }

    pub(crate) fn finalize(&mut self, summary: &RecordingSummary, ended_at_utc: DateTime<Utc>) {
        self.update_from_summary(summary);
        self.timestamps.set_ended(ended_at_utc);
        self.refresh_durations(ended_at_utc);
    }

    pub(crate) fn mark_saved(&mut self, saved_at_utc: DateTime<Utc>) {
        self.timestamps.set_saved(saved_at_utc);
        self.refresh_durations(saved_at_utc);
    }

    pub(crate) fn suggested_filename(&self, format: RecordingFormat) -> String {
        let end = self.timestamps.ended_at_utc.unwrap_or_else(Utc::now);
        format!(
            "KM003C_{}_to_{}_BJT_{}SPS.{}",
            format_beijing_compact(self.timestamps.started_at_utc),
            format_beijing_compact(end),
            self.sample_rate_hz,
            format.extension(),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct RecordingSegmentMetadata {
    pub(crate) index: u32,
    pub(crate) file_name: String,
    pub(crate) start_row: u64,
    pub(crate) end_row: u64,
    pub(crate) start_elapsed_us: u64,
    pub(crate) end_elapsed_us: u64,
    pub(crate) sealed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct RecordingSessionManifestV1 {
    pub(crate) schema_version: u32,
    pub(crate) state: SessionState,
    pub(crate) format: RecordingFormat,
    pub(crate) metadata: RecordingSessionMetadataV1,
    pub(crate) segments: Vec<RecordingSegmentMetadata>,
}

impl RecordingSessionManifestV1 {
    pub(crate) fn new(format: RecordingFormat, metadata: RecordingSessionMetadataV1) -> Self {
        Self {
            schema_version: SESSION_SCHEMA_VERSION,
            state: SessionState::Recording,
            format,
            metadata,
            segments: Vec::new(),
        }
    }
}

pub(crate) fn format_beijing(value: DateTime<Utc>) -> String {
    value
        .with_timezone(&Shanghai)
        .format("%Y-%m-%d %H:%M:%S%.3f BJT")
        .to_string()
}

fn format_beijing_compact(value: DateTime<Utc>) -> String {
    value.with_timezone(&Shanghai).format("%Y%m%d-%H%M%S").to_string()
}

fn create_session_id(value: DateTime<Utc>) -> String {
    format!(
        "{}-{:09}-{}",
        value.format("%Y%m%dT%H%M%S"),
        value.timestamp_subsec_nanos(),
        std::process::id(),
    )
}

pub(crate) fn sidecar_path(data_path: &Path) -> PathBuf {
    let mut sidecar = data_path.as_os_str().to_os_string();
    sidecar.push(".km003c.json");
    PathBuf::from(sidecar)
}

pub(crate) fn write_sidecar(data_path: &Path, metadata: &RecordingSessionMetadataV1) -> Result<PathBuf, String> {
    let sidecar = sidecar_path(data_path);
    write_json_atomically(&sidecar, metadata)?;
    Ok(sidecar)
}

pub(crate) fn read_sidecar(data_path: &Path) -> Result<Option<RecordingSessionMetadataV1>, String> {
    let sidecar = sidecar_path(data_path);
    if !sidecar.is_file() {
        return Ok(None);
    }
    let bytes = fs::read(&sidecar).map_err(|error| format!("无法读取元数据 {}：{error}", sidecar.display()))?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| format!("元数据格式错误 {}：{error}", sidecar.display()))
}

pub(crate) fn write_manifest(
    session_directory: &Path,
    manifest: &RecordingSessionManifestV1,
) -> Result<PathBuf, String> {
    let path = session_directory.join("manifest.json");
    write_json_atomically(&path, manifest)?;
    Ok(path)
}

pub(crate) fn read_manifest(path: &Path) -> Result<RecordingSessionManifestV1, String> {
    let bytes = fs::read(path).map_err(|error| format!("无法读取恢复清单 {}：{error}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("恢复清单格式错误 {}：{error}", path.display()))
}

pub(crate) fn discover_recoverable_sessions(pending_directory: &Path) -> Vec<(PathBuf, RecordingSessionManifestV1)> {
    let mut sessions = fs::read_dir(pending_directory)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .filter_map(|directory| {
            let manifest_path = directory.join("manifest.json");
            read_manifest(&manifest_path).ok().map(|manifest| (directory, manifest))
        })
        .collect::<Vec<_>>();
    sessions.sort_by(|(_, left), (_, right)| {
        right
            .metadata
            .timestamps
            .started_at_utc
            .cmp(&left.metadata.timestamps.started_at_utc)
    });
    sessions
}

pub(crate) fn merge_session_segments(
    session_directory: &Path,
    manifest: &RecordingSessionManifestV1,
    destination: &Path,
) -> Result<RecordingSummary, String> {
    let segment_paths = manifest
        .segments
        .iter()
        .map(|segment| session_directory.join("segments").join(&segment.file_name))
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    if segment_paths.is_empty() {
        return Err("恢复会话中没有可合并的数据段".to_string());
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("无法创建保存目录 {}：{error}", parent.display()))?;
    }
    let temporary = append_suffix(destination, ".partial");
    match manifest.format {
        RecordingFormat::Csv => merge_csv_segments(&segment_paths, &temporary)?,
        RecordingFormat::Parquet => merge_parquet_segments(&segment_paths, &temporary, &manifest.metadata)?,
    }
    let file = File::open(&temporary).map_err(|error| format!("无法校验合并文件：{error}"))?;
    file.sync_all().map_err(|error| format!("无法同步合并文件：{error}"))?;
    if destination.exists() {
        fs::remove_file(destination).map_err(|error| format!("无法替换已有文件：{error}"))?;
    }
    fs::rename(&temporary, destination).map_err(|error| format!("无法完成合并文件：{error}"))?;
    summarize_merged_recording(destination, manifest.format)
}

fn summarize_merged_recording(path: &Path, format: RecordingFormat) -> Result<RecordingSummary, String> {
    let frame = match format {
        RecordingFormat::Csv => CsvReader::new(
            File::open(path).map_err(|error| format!("无法打开合并后的 CSV {}：{error}", path.display()))?,
        )
        .finish()
        .map_err(|error| format!("无法校验合并后的 CSV {}：{error}", path.display()))?,
        RecordingFormat::Parquet => ParquetReader::new(
            File::open(path).map_err(|error| format!("无法打开合并后的 Parquet {}：{error}", path.display()))?,
        )
        .finish()
        .map_err(|error| format!("无法校验合并后的 Parquet {}：{error}", path.display()))?,
    };
    let last = frame
        .height()
        .checked_sub(1)
        .ok_or_else(|| "合并后的录制文件没有采样点".to_string())?;
    Ok(RecordingSummary {
        path: path.to_path_buf(),
        rows: frame.height() as u64,
        elapsed_us: last_u64(&frame, "elapsed_us", last)?,
        missing_samples: last_u64(&frame, "cumulative_missing_samples", last)?,
        interpolated_duration_us: last_u64(&frame, "cumulative_interpolated_duration_us", last)?,
        discarded_sequence_samples: last_u64(&frame, "cumulative_discarded_sequence_samples", last)?,
        charge_uah: last_f64(&frame, "charge_uah", last)?,
        energy_uwh: last_f64(&frame, "energy_uwh", last)?,
        charge_throughput_uah: last_f64(&frame, "charge_throughput_uah", last)?,
        energy_throughput_uwh: last_f64(&frame, "energy_throughput_uwh", last)?,
    })
}

fn last_u64(frame: &DataFrame, name: &str, row: usize) -> Result<u64, String> {
    let column = frame
        .column(name)
        .map_err(|_| format!("合并录制缺少字段：{name}"))?
        .cast(&DataType::UInt64)
        .map_err(|error| format!("合并录制字段 {name} 无法转换为整数：{error}"))?;
    column
        .u64()
        .map_err(|error| format!("合并录制字段 {name} 类型错误：{error}"))?
        .get(row)
        .ok_or_else(|| format!("合并录制字段 {name} 末行为空"))
}

fn last_f64(frame: &DataFrame, name: &str, row: usize) -> Result<f64, String> {
    let column = frame
        .column(name)
        .map_err(|_| format!("合并录制缺少字段：{name}"))?
        .cast(&DataType::Float64)
        .map_err(|error| format!("合并录制字段 {name} 无法转换为数值：{error}"))?;
    column
        .f64()
        .map_err(|error| format!("合并录制字段 {name} 类型错误：{error}"))?
        .get(row)
        .ok_or_else(|| format!("合并录制字段 {name} 末行为空"))
}

fn merge_csv_segments(segment_paths: &[PathBuf], destination: &Path) -> Result<(), String> {
    let mut output =
        BufWriter::new(File::create(destination).map_err(|error| format!("无法创建 CSV 合并文件：{error}"))?);
    for (index, path) in segment_paths.iter().enumerate() {
        let file = File::open(path).map_err(|error| format!("无法打开数据段 {}：{error}", path.display()))?;
        let mut input = BufReader::new(file);
        if index > 0 {
            let mut header = String::new();
            input
                .read_line(&mut header)
                .map_err(|error| format!("无法读取 CSV 数据段表头 {}：{error}", path.display()))?;
        }
        std::io::copy(&mut input, &mut output)
            .map_err(|error| format!("无法合并 CSV 数据段 {}：{error}", path.display()))?;
    }
    output
        .flush()
        .map_err(|error| format!("无法写完 CSV 合并文件：{error}"))
}

fn merge_parquet_segments(
    segment_paths: &[PathBuf],
    destination: &Path,
    metadata: &RecordingSessionMetadataV1,
) -> Result<(), String> {
    let first = ParquetReader::new(
        File::open(&segment_paths[0])
            .map_err(|error| format!("无法打开 Parquet 数据段 {}：{error}", segment_paths[0].display()))?,
    )
    .finish()
    .map_err(|error| format!("无法读取 Parquet 数据段 {}：{error}", segment_paths[0].display()))?;
    let metadata_json =
        serde_json::to_string(metadata).map_err(|error| format!("无法序列化 Parquet 会话元数据：{error}"))?;
    let key_values = KeyValueMetadata::from_static(vec![
        ("km003c.schema_version".to_string(), "1".to_string()),
        ("km003c.source".to_string(), "recording_session".to_string()),
        (PARQUET_SESSION_METADATA_KEY.to_string(), metadata_json),
    ]);
    let mut writer = ParquetWriter::new(BufWriter::new(
        File::create(destination).map_err(|error| format!("无法创建 Parquet 合并文件：{error}"))?,
    ))
    .with_key_value_metadata(Some(key_values))
    .with_row_group_size(Some(8_192))
    .batched(first.schema())
    .map_err(|error| format!("无法初始化 Parquet 合并器：{error}"))?;
    writer
        .write_batch(&first)
        .map_err(|error| format!("无法写入 Parquet 数据段 {}：{error}", segment_paths[0].display()))?;
    for path in &segment_paths[1..] {
        let frame = ParquetReader::new(
            File::open(path).map_err(|error| format!("无法打开 Parquet 数据段 {}：{error}", path.display()))?,
        )
        .finish()
        .map_err(|error| format!("无法读取 Parquet 数据段 {}：{error}", path.display()))?;
        writer
            .write_batch(&frame)
            .map_err(|error| format!("无法写入 Parquet 数据段 {}：{error}", path.display()))?;
    }
    writer
        .finish()
        .map_err(|error| format!("无法结束 Parquet 合并文件：{error}"))?;
    Ok(())
}

#[expect(
    dead_code,
    reason = "used when wiring crash-tail recovery into the session recovery UI"
)]
pub(crate) fn recover_csv_partial(partial_path: &Path, recovered_path: &Path) -> Result<u64, String> {
    let mut bytes = Vec::new();
    File::open(partial_path)
        .map_err(|error| format!("无法打开 CSV 尾段 {}：{error}", partial_path.display()))?
        .read_to_end(&mut bytes)
        .map_err(|error| format!("无法读取 CSV 尾段 {}：{error}", partial_path.display()))?;
    let complete_len = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |position| position + 1);
    if complete_len == 0 {
        return Err("CSV 尾段没有完整行".to_string());
    }
    let mut output = File::create(recovered_path)
        .map_err(|error| format!("无法创建 CSV 恢复段 {}：{error}", recovered_path.display()))?;
    output
        .write_all(&bytes[..complete_len])
        .map_err(|error| format!("无法写入 CSV 恢复段 {}：{error}", recovered_path.display()))?;
    output
        .sync_all()
        .map_err(|error| format!("无法同步 CSV 恢复段 {}：{error}", recovered_path.display()))?;
    let rows = CsvReader::new(
        File::open(recovered_path)
            .map_err(|error| format!("无法校验 CSV 恢复段 {}：{error}", recovered_path.display()))?,
    )
    .finish()
    .map_err(|error| format!("CSV 恢复段校验失败 {}：{error}", recovered_path.display()))?
    .height() as u64;
    Ok(rows)
}

fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

pub(crate) fn write_json_atomically<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|error| format!("无法创建目录 {}：{error}", parent.display()))?;
    let temporary = path.with_extension(format!(
        "{}.tmp-{}",
        path.extension().and_then(|value| value.to_str()).unwrap_or("json"),
        std::process::id(),
    ));
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| format!("无法序列化录制元数据：{error}"))?;
    let mut file =
        File::create(&temporary).map_err(|error| format!("无法创建临时元数据 {}：{error}", temporary.display()))?;
    file.write_all(&bytes)
        .map_err(|error| format!("无法写入临时元数据 {}：{error}", temporary.display()))?;
    file.sync_all()
        .map_err(|error| format!("无法同步临时元数据 {}：{error}", temporary.display()))?;
    fs::rename(&temporary, path).map_err(|error| format!("无法原子更新元数据 {}：{error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use polars::{
        df,
        prelude::{CsvWriter, SerWriter},
    };
    use std::io::BufWriter;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_PATH: AtomicU64 = AtomicU64::new(0);

    fn temp_path(name: &str) -> PathBuf {
        let sequence = NEXT_PATH.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("km003c-session-{}-{sequence}-{name}", std::process::id()))
    }

    #[test]
    fn beijing_filename_and_display_cross_utc_day_correctly() {
        let started = Utc.with_ymd_and_hms(2026, 8, 31, 16, 56, 13).unwrap();
        let ended = Utc.with_ymd_and_hms(2026, 8, 31, 17, 8, 42).unwrap();
        let mut metadata = RecordingSessionMetadataV1::new(started, RecordingMetadata::default(), 50);
        metadata.timestamps.set_ended(ended);

        assert_eq!(metadata.timestamps.started_at_beijing, "2026-09-01 00:56:13.000 BJT");
        assert_eq!(
            metadata.suggested_filename(RecordingFormat::Csv),
            "KM003C_20260901-005613_to_20260901-010842_BJT_50SPS.csv"
        );
    }

    #[test]
    fn sidecar_round_trip_preserves_utc_and_beijing_times() {
        let data_path = temp_path("capture.csv");
        let started = Utc.with_ymd_and_hms(2026, 8, 31, 2, 56, 13).unwrap();
        let metadata = RecordingSessionMetadataV1::new(started, RecordingMetadata::default(), 1_000);
        let sidecar = write_sidecar(&data_path, &metadata).unwrap();
        let restored = read_sidecar(&data_path).unwrap().unwrap();

        assert_eq!(restored.session_id, metadata.session_id);
        assert_eq!(restored.timestamps.started_at_utc, started);
        assert_eq!(restored.timestamps.display_timezone, DISPLAY_TIMEZONE);
        fs::remove_file(sidecar).unwrap();
    }

    #[test]
    fn interval_duration_never_becomes_negative() {
        let start = Utc.with_ymd_and_hms(2026, 8, 31, 3, 0, 0).unwrap();
        let interval = RecordingTimeInterval {
            reason: IntervalReason::UsbDisconnected,
            started_at_utc: start,
            ended_at_utc: Some(start - chrono::Duration::seconds(1)),
        };
        assert_eq!(interval.duration_ms(start), 0);
    }

    #[test]
    fn recovery_uses_segment_contents_when_manifest_was_not_updated_before_sleep() {
        let directory = temp_path("stale-manifest");
        let segments = directory.join("segments");
        fs::create_dir_all(&segments).unwrap();
        let segment_name = "segment-000000.csv";
        let segment_path = segments.join(segment_name);
        let mut frame = df!(
            "elapsed_us" => vec![0_u64, 25_100_000],
            "cumulative_missing_samples" => vec![0_u64, 2],
            "cumulative_interpolated_duration_us" => vec![0_u64, 20_000],
            "cumulative_discarded_sequence_samples" => vec![0_u64, 1],
            "charge_uah" => vec![0.0_f64, 6_958.0],
            "energy_uwh" => vec![0.0_f64, 34_285.0],
            "charge_throughput_uah" => vec![0.0_f64, 6_958.0],
            "energy_throughput_uwh" => vec![0.0_f64, 34_285.0],
        )
        .unwrap();
        CsvWriter::new(BufWriter::new(File::create(&segment_path).unwrap()))
            .finish(&mut frame)
            .unwrap();

        let mut manifest = RecordingSessionManifestV1::new(RecordingFormat::Csv, RecordingSessionMetadataV1::default());
        manifest.segments.push(RecordingSegmentMetadata {
            index: 0,
            file_name: segment_name.to_string(),
            start_row: 0,
            end_row: 0,
            start_elapsed_us: 0,
            end_elapsed_us: 0,
            sealed: false,
        });
        let destination = directory.join("recovered.csv");

        let summary = merge_session_segments(&directory, &manifest, &destination).unwrap();

        assert_eq!(summary.rows, 2);
        assert_eq!(summary.elapsed_us, 25_100_000);
        assert_eq!(summary.charge_throughput_uah, 6_958.0);
        assert_eq!(summary.energy_throughput_uwh, 34_285.0);
        fs::remove_dir_all(directory).unwrap();
    }
}
