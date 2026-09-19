//! Read-only snapshot of offline metadata and the exact advertised sample bytes.
//! Close other KM003C clients first. Does not reset, erase, or write device memory.
use km003c_lib::{DeviceConfig, KM003C};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut device = KM003C::new(DeviceConfig::vendor().skip_reset()).await?;
    let catalog = device.request_log_metadata().await?;
    for metadata in catalog {
        println!("METADATA {}", hex::encode(metadata.to_bytes()));
        println!("DETAIL {metadata:?}");
        let bytes = device
            .read_memory_block(metadata.data_address()?, metadata.data_size())
            .await?;
        println!("SAMPLES {}", hex::encode(bytes));
    }
    Ok(())
}
