use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread::{self, JoinHandle};

use polars::prelude::{CsvReader, DataFrame, DataType, ParquetReader, SerReader};

use crate::measurement::MeasurementSample;
use crate::recording_session::{PARQUET_SESSION_METADATA_KEY, RecordingSessionMetadataV1, read_sidecar};

const RECORDING_COLUMNS: [&str; 23] = [
    "elapsed_us",
    "sample_index",
    "sequence",
    "marker",
    "sample_rate_hz",
    "missing_samples",
    "gap_duration_us",
    "interpolated",
    "cumulative_missing_samples",
    "cumulative_interpolated_duration_us",
    "discarded_sequence_samples",
    "cumulative_discarded_sequence_samples",
    "vbus_uv",
    "ibus_ua",
    "power_uw",
    "charge_uah",
    "energy_uwh",
    "charge_throughput_uah",
    "energy_throughput_uwh",
    "cc1_uv",
    "cc2_uv",
    "dp_uv",
    "dm_uv",
];

#[derive(Debug, Clone)]
pub(crate) struct ImportedRecording {
    pub(crate) path: PathBuf,
    pub(crate) samples: Arc<Vec<MeasurementSample>>,
    pub(crate) metadata: Option<RecordingSessionMetadataV1>,
}

#[derive(Debug)]
pub(crate) enum RecordingImportEvent {
    Finished(Box<ImportedRecording>),
    Failed(String),
}

pub(crate) struct RecordingImportTask {
    event_rx: Receiver<RecordingImportEvent>,
    handle: Option<JoinHandle<()>>,
}

impl RecordingImportTask {
    pub(crate) fn start(path: PathBuf) -> Result<Self, String> {
        if !path.is_file() {
            return Err(format!("文件不存在：{}", path.display()));
        }
        let worker_path = path.clone();
        let (event_tx, event_rx) = mpsc::channel();
        let handle = thread::Builder::new()
            .name("km003c-recording-import".to_string())
            .spawn(move || {
                let event = match load_recording(&worker_path) {
                    Ok(recording) => RecordingImportEvent::Finished(Box::new(recording)),
                    Err(error) => RecordingImportEvent::Failed(error),
                };
                let _ = event_tx.send(event);
            })
            .map_err(|error| format!("无法启动导入任务：{error}"))?;
        Ok(Self {
            event_rx,
            handle: Some(handle),
        })
    }

    pub(crate) fn poll_event(&mut self) -> Option<RecordingImportEvent> {
        match self.event_rx.try_recv() {
            Ok(event) => {
                if let Some(handle) = self.handle.take() {
                    let _ = handle.join();
                }
                Some(event)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                Some(RecordingImportEvent::Failed("导入任务意外退出，未返回结果".to_string()))
            }
        }
    }
}

impl Drop for RecordingImportTask {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

pub(crate) fn load_recording(path: &Path) -> Result<ImportedRecording, String> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| "文件没有扩展名，只支持 .csv 和 .parquet".to_string())?;
    let metadata = read_sidecar(path)?;
    let frame = match extension.as_str() {
        "csv" => CsvReader::new(File::open(path).map_err(display_error)?),
        "parquet" => {
            return load_parquet(path, metadata);
        }
        _ => return Err("文件格式不支持，只能导入 KM003C CSV 或 Parquet".to_string()),
    }
    .finish()
    .map_err(|error| format!("CSV 读取失败：{error}"))?;
    dataframe_to_recording(path, frame, metadata)
}

fn load_parquet(
    path: &Path,
    sidecar_metadata: Option<RecordingSessionMetadataV1>,
) -> Result<ImportedRecording, String> {
    let mut reader = ParquetReader::new(File::open(path).map_err(display_error)?);
    let embedded_metadata = if sidecar_metadata.is_none() {
        let file_metadata = reader
            .get_metadata()
            .map_err(|error| format!("Parquet 元数据读取失败：{error}"))?;
        file_metadata
            .key_value_metadata()
            .as_ref()
            .and_then(|values| values.iter().find(|value| value.key == PARQUET_SESSION_METADATA_KEY))
            .and_then(|value| value.value.as_deref())
            .map(|json| {
                serde_json::from_str::<RecordingSessionMetadataV1>(json)
                    .map_err(|error| format!("Parquet 内嵌 KM003C 元数据格式错误：{error}"))
            })
            .transpose()?
    } else {
        None
    };
    let frame = reader.finish().map_err(|error| format!("Parquet 读取失败：{error}"))?;
    dataframe_to_recording(path, frame, sidecar_metadata.or(embedded_metadata))
}

fn dataframe_to_recording(
    path: &Path,
    frame: DataFrame,
    metadata: Option<RecordingSessionMetadataV1>,
) -> Result<ImportedRecording, String> {
    if frame.width() != RECORDING_COLUMNS.len() {
        return Err(format!(
            "列数不匹配：需要 KM003C 的 23 列，文件中有 {} 列",
            frame.width()
        ));
    }
    for name in RECORDING_COLUMNS {
        if frame.column(name).is_err() {
            return Err(format!("缺少 KM003C 字段：{name}"));
        }
    }
    if frame.height() == 0 {
        return Err("录制文件没有采样点".to_string());
    }

    let elapsed_us = required_u64(&frame, "elapsed_us")?;
    let sample_index = required_u64(&frame, "sample_index")?;
    let sequence = optional_u32(&frame, "sequence")?;
    let marker = optional_u32(&frame, "marker")?;
    let sample_rate_hz = optional_u32(&frame, "sample_rate_hz")?;
    let missing_samples = optional_u32(&frame, "missing_samples")?;
    let gap_duration_us = optional_u64(&frame, "gap_duration_us")?;
    let interpolated = optional_bool(&frame, "interpolated")?;
    let cumulative_missing_samples = optional_u64(&frame, "cumulative_missing_samples")?;
    let cumulative_interpolated_duration_us = optional_u64(&frame, "cumulative_interpolated_duration_us")?;
    let discarded_sequence_samples = optional_u32(&frame, "discarded_sequence_samples")?;
    let cumulative_discarded_sequence_samples = optional_u64(&frame, "cumulative_discarded_sequence_samples")?;
    let vbus_uv = required_i64(&frame, "vbus_uv")?;
    let ibus_ua = required_i64(&frame, "ibus_ua")?;
    let power_uw = required_i64(&frame, "power_uw")?;
    let charge_uah = required_f64(&frame, "charge_uah")?;
    let energy_uwh = required_f64(&frame, "energy_uwh")?;
    let charge_throughput_uah = required_f64(&frame, "charge_throughput_uah")?;
    let energy_throughput_uwh = required_f64(&frame, "energy_throughput_uwh")?;
    let cc1_uv = optional_i64(&frame, "cc1_uv")?;
    let cc2_uv = optional_i64(&frame, "cc2_uv")?;
    let dp_uv = optional_i64(&frame, "dp_uv")?;
    let dm_uv = optional_i64(&frame, "dm_uv")?;

    let mut samples = Vec::with_capacity(frame.height());
    let mut previous_time = None;
    for row in 0..frame.height() {
        let time = elapsed_us[row];
        if previous_time.is_some_and(|previous| time < previous) {
            return Err(format!("时间字段乱序：第 {} 行的 elapsed_us 小于前一行", row + 2));
        }
        previous_time = Some(time);
        samples.push(MeasurementSample {
            elapsed_us: time,
            sample_index: sample_index[row],
            sequence: sequence[row].min(u32::from(u16::MAX)) as u16,
            marker: marker[row].min(u32::from(u16::MAX)) as u16,
            sample_rate_hz: sample_rate_hz[row].min(u32::from(u16::MAX)) as u16,
            missing_samples: missing_samples[row].min(u32::from(u16::MAX)) as u16,
            gap_duration_us: gap_duration_us[row],
            interpolated: interpolated[row],
            cumulative_missing_samples: cumulative_missing_samples[row],
            cumulative_interpolated_duration_us: cumulative_interpolated_duration_us[row],
            discarded_sequence_samples: discarded_sequence_samples[row],
            cumulative_discarded_sequence_samples: cumulative_discarded_sequence_samples[row],
            vbus_uv: vbus_uv[row],
            ibus_ua: ibus_ua[row],
            power_uw: power_uw[row],
            charge_uah: charge_uah[row],
            energy_uwh: energy_uwh[row],
            charge_throughput_uah: charge_throughput_uah[row],
            energy_throughput_uwh: energy_throughput_uwh[row],
            cc1_uv: cc1_uv[row],
            cc2_uv: cc2_uv[row],
            dp_uv: dp_uv[row],
            dm_uv: dm_uv[row],
        });
    }

    Ok(ImportedRecording {
        path: path.to_path_buf(),
        samples: Arc::new(samples),
        metadata,
    })
}

fn display_error(error: std::io::Error) -> String {
    format!("无法打开文件：{error}")
}

fn required_u64(frame: &DataFrame, name: &str) -> Result<Vec<u64>, String> {
    let cast = cast_column(frame, name, DataType::UInt64)?;
    cast.u64()
        .map_err(|error| format!("字段 {name} 类型错误：{error}"))?
        .iter()
        .enumerate()
        .map(|(row, value)| value.ok_or_else(|| format!("字段 {name} 第 {} 行为空", row + 2)))
        .collect()
}

fn optional_u64(frame: &DataFrame, name: &str) -> Result<Vec<u64>, String> {
    let cast = cast_column(frame, name, DataType::UInt64)?;
    Ok(cast
        .u64()
        .map_err(|error| format!("字段 {name} 类型错误：{error}"))?
        .iter()
        .map(|value| value.unwrap_or_default())
        .collect())
}

fn optional_u32(frame: &DataFrame, name: &str) -> Result<Vec<u32>, String> {
    let cast = cast_column(frame, name, DataType::UInt32)?;
    Ok(cast
        .u32()
        .map_err(|error| format!("字段 {name} 类型错误：{error}"))?
        .iter()
        .map(|value| value.unwrap_or_default())
        .collect())
}

fn required_i64(frame: &DataFrame, name: &str) -> Result<Vec<i64>, String> {
    let cast = cast_column(frame, name, DataType::Int64)?;
    cast.i64()
        .map_err(|error| format!("字段 {name} 类型错误：{error}"))?
        .iter()
        .enumerate()
        .map(|(row, value)| value.ok_or_else(|| format!("字段 {name} 第 {} 行为空", row + 2)))
        .collect()
}

fn optional_i64(frame: &DataFrame, name: &str) -> Result<Vec<i64>, String> {
    let cast = cast_column(frame, name, DataType::Int64)?;
    Ok(cast
        .i64()
        .map_err(|error| format!("字段 {name} 类型错误：{error}"))?
        .iter()
        .map(|value| value.unwrap_or_default())
        .collect())
}

fn required_f64(frame: &DataFrame, name: &str) -> Result<Vec<f64>, String> {
    let cast = cast_column(frame, name, DataType::Float64)?;
    cast.f64()
        .map_err(|error| format!("字段 {name} 类型错误：{error}"))?
        .iter()
        .enumerate()
        .map(|(row, value)| value.ok_or_else(|| format!("字段 {name} 第 {} 行为空", row + 2)))
        .collect()
}

fn optional_bool(frame: &DataFrame, name: &str) -> Result<Vec<bool>, String> {
    let cast = cast_column(frame, name, DataType::Boolean)?;
    Ok(cast
        .bool()
        .map_err(|error| format!("字段 {name} 类型错误：{error}"))?
        .iter()
        .map(|value| value.unwrap_or_default())
        .collect())
}

fn cast_column(frame: &DataFrame, name: &str, data_type: DataType) -> Result<polars::prelude::Column, String> {
    frame
        .column(name)
        .map_err(|_| format!("缺少 KM003C 字段：{name}"))?
        .cast(&data_type)
        .map_err(|error| format!("字段 {name} 无法转换为 {data_type:?}：{error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use polars::df;
    use polars::prelude::{CsvWriter, SerWriter};
    use std::io::BufWriter;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

    fn test_path() -> PathBuf {
        let sequence = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("km003c-recording-import-{}-{sequence}.csv", std::process::id()))
    }

    fn valid_frame(times: Vec<u64>) -> DataFrame {
        let rows = times.len();
        df!(
            "elapsed_us" => times,
            "sample_index" => (0..rows as u64).collect::<Vec<_>>(),
            "sequence" => vec![1_u32; rows],
            "marker" => vec![0_u32; rows],
            "sample_rate_hz" => vec![50_u32; rows],
            "missing_samples" => vec![0_u32; rows],
            "gap_duration_us" => vec![0_u64; rows],
            "interpolated" => vec![false; rows],
            "cumulative_missing_samples" => vec![0_u64; rows],
            "cumulative_interpolated_duration_us" => vec![0_u64; rows],
            "discarded_sequence_samples" => vec![0_u32; rows],
            "cumulative_discarded_sequence_samples" => vec![0_u64; rows],
            "vbus_uv" => vec![9_000_000_i64; rows],
            "ibus_ua" => vec![2_000_000_i64; rows],
            "power_uw" => vec![18_000_000_i64; rows],
            "charge_uah" => vec![1.0_f64; rows],
            "energy_uwh" => vec![2.0_f64; rows],
            "charge_throughput_uah" => vec![1.0_f64; rows],
            "energy_throughput_uwh" => vec![2.0_f64; rows],
            "cc1_uv" => vec![600_000_i64; rows],
            "cc2_uv" => vec![0_i64; rows],
            "dp_uv" => vec![500_000_i64; rows],
            "dm_uv" => vec![500_000_i64; rows],
        )
        .unwrap()
    }

    fn write_csv(path: &Path, mut frame: DataFrame) {
        CsvWriter::new(BufWriter::new(File::create(path).unwrap()))
            .finish(&mut frame)
            .unwrap();
    }

    #[test]
    fn imports_the_stable_23_column_schema() {
        let path = test_path();
        write_csv(&path, valid_frame(vec![0, 20_000]));

        let imported = load_recording(&path).unwrap();

        assert_eq!(imported.samples.len(), 2);
        assert_eq!(imported.samples[1].elapsed_us, 20_000);
        assert_eq!(imported.samples[1].power_uw, 18_000_000);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rejects_missing_columns_and_out_of_order_time() {
        let missing_path = test_path();
        let mut missing = valid_frame(vec![0]);
        missing.drop_in_place("dm_uv").unwrap();
        write_csv(&missing_path, missing);
        assert!(load_recording(&missing_path).unwrap_err().contains("23 列"));

        let disorder_path = test_path();
        write_csv(&disorder_path, valid_frame(vec![20_000, 10_000]));
        assert!(load_recording(&disorder_path).unwrap_err().contains("时间字段乱序"));

        let _ = std::fs::remove_file(missing_path);
        let _ = std::fs::remove_file(disorder_path);
    }
}
