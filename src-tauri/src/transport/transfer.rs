use anyhow::{bail, Result};
use std::path::Path;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Partial file extension for in-progress transfers.
const PARTIAL_EXT: &str = ".lan-sync-partial";

/// Maximum retry count for failed transfers.
const MAX_RETRIES: u32 = 3;

/// Send a file over a TCP stream.
///
/// Protocol:
/// 1. Send 8 bytes: file size (big-endian u64)
/// 2. Send file data
/// 3. Send 32 bytes: blake3 hash
///
/// Returns the blake3 hash of the sent data.
pub async fn send_file<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    file_path: &Path,
) -> Result<String> {
    let data = tokio::fs::read(file_path).await?;
    let hash = blake3::hash(&data);
    let hash_hex = hash.to_hex().to_string();

    // Send size
    let size = data.len() as u64;
    writer.write_all(&size.to_be_bytes()).await?;

    // Send data
    writer.write_all(&data).await?;

    // Send hash
    writer.write_all(hash.as_bytes()).await?;

    writer.flush().await?;
    Ok(hash_hex)
}

/// Receive a file from a TCP stream.
///
/// Writes to a `.lan-sync-partial` temporary file first,
/// verifies the hash, then atomically renames to the final path.
pub async fn receive_file<R: AsyncReadExt + Unpin>(
    reader: &mut R,
    target_path: &Path,
) -> Result<String> {
    // Ensure parent directory exists
    if let Some(parent) = target_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    // Read size
    let mut size_buf = [0u8; 8];
    reader.read_exact(&mut size_buf).await?;
    let size = u64::from_be_bytes(size_buf);

    // Read data
    let mut data = vec![0u8; size as usize];
    reader.read_exact(&mut data).await?;

    // Read expected hash
    let mut hash_buf = [0u8; 32];
    reader.read_exact(&mut hash_buf).await?;
    let expected_hash = blake3::Hash::from(hash_buf);

    // Compute actual hash
    let actual_hash = blake3::hash(&data);

    if actual_hash != expected_hash {
        bail!(
            "hash mismatch: expected {}, got {}",
            expected_hash,
            actual_hash
        );
    }

    // Write to partial file
    let partial_path = format!("{}{}", target_path.to_string_lossy(), PARTIAL_EXT);
    tokio::fs::write(&partial_path, &data).await?;

    // Verify written data
    let written = tokio::fs::read(&partial_path).await?;
    let written_hash = blake3::hash(&written);
    if written_hash != actual_hash {
        tokio::fs::remove_file(&partial_path).await?;
        bail!("written file hash mismatch after write");
    }

    // Atomic rename
    tokio::fs::rename(&partial_path, target_path).await?;

    Ok(actual_hash.to_hex().to_string())
}

/// Clean up stale partial files in a directory.
///
/// Called on startup to handle interrupted transfers.
pub async fn cleanup_partial_files(dir: &Path) -> Result<usize> {
    let mut cleaned = 0;
    let mut entries = tokio::fs::read_dir(dir).await?;

    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if let Some(ext) = path.extension() {
            if ext == PARTIAL_EXT.trim_start_matches('.') {
                tokio::fs::remove_file(&path).await?;
                cleaned += 1;
                tracing::info!("cleaned stale partial file: {}", path.display());
            }
        }
    }

    Ok(cleaned)
}

/// Send a file with retry logic.
///
/// Retries network/temporary errors up to MAX_RETRIES times
/// with 1s, 2s, 4s exponential backoff.
pub async fn send_file_with_retry<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    file_path: &Path,
) -> Result<String> {
    let mut last_error = None;

    for attempt in 0..MAX_RETRIES {
        match send_file(writer, file_path).await {
            Ok(hash) => return Ok(hash),
            Err(e) => {
                last_error = Some(e);
                if attempt < MAX_RETRIES - 1 {
                    let delay = 2u64.pow(attempt);
                    tracing::warn!(
                        "transfer attempt {} failed, retrying in {}s",
                        attempt + 1,
                        delay
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
                }
            }
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("transfer failed after retries")))
}
