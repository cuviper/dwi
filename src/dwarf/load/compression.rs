use std::{fs, io::Read, path::Path};

use anyhow::{Context, Result};

use super::LoadLimits;

pub(super) fn read_object(path: &Path, limits: LoadLimits) -> Result<Vec<u8>> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    decode(bytes, limits).with_context(|| format!("reading object file {}", path.display()))
}

fn decode(bytes: Vec<u8>, limits: LoadLimits) -> Result<Vec<u8>> {
    let size = bytes.len() as u64;
    let limit = limits.expansion_limit(size);
    let mut decoder: Box<dyn Read + '_> = if bytes.starts_with(b"\x1f\x8b") {
        Box::new(flate2::read::MultiGzDecoder::new(bytes.as_slice()))
    } else if bytes.starts_with(b"\xfd7zXZ\0") {
        // Keep decoder workspace proportional too, allowing an extra MiB for
        // xz's bookkeeping beyond the LZMA dictionary itself.
        let memory_kib = limit
            .div_ceil(1024)
            .saturating_add(1024)
            .min(u32::MAX as u64) as u32;
        Box::new(lzma_rust2::XzReader::new_mem_limit(
            bytes.as_slice(),
            true,
            memory_kib,
        ))
    } else if bytes.starts_with(b"\x28\xb5\x2f\xfd") {
        Box::new(
            ruzstd::decoding::StreamingDecoder::new_with_max_window_size(bytes.as_slice(), limit)
                .context("reading zstd frame")?,
        )
    } else {
        return Ok(bytes);
    };
    let mut decoded = Vec::new();
    decoder
        .by_ref()
        .take(limit.saturating_add(1))
        .read_to_end(&mut decoded)
        .context("decompressing object (workspace can be raised with --max-decompression-ratio)")?;
    limits.check_expansion(size, decoded.len() as u64)?;
    Ok(decoded)
}

#[cfg(test)]
pub(super) mod tests;
