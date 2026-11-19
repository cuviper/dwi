use std::{
    borrow::Cow,
    fmt,
    path::{Path, PathBuf},
    rc::Rc,
};

use anyhow::{Context, Result, ensure};
use object::{CompressionFormat, ObjectSection};

/// Guard against disproportionate expansion of compressed debug sections.
#[derive(Clone, Copy, Debug)]
pub struct LoadLimits {
    pub max_decompression_ratio: u64,
}

impl LoadLimits {
    pub const DEFAULT_MAX_DECOMPRESSION_RATIO: u64 = 256;
    const SMALL_SECTION_ALLOWANCE: u64 = 16 * 1024 * 1024;

    pub(super) fn expansion_limit(self, compressed: u64) -> u64 {
        compressed
            .saturating_mul(self.max_decompression_ratio)
            .max(Self::SMALL_SECTION_ALLOWANCE)
    }

    pub(super) fn check_expansion(self, compressed: u64, decoded: u64) -> Result<()> {
        ensure!(
            decoded <= self.expansion_limit(compressed),
            ExpansionExceeded {
                compressed,
                decoded,
                ratio: self.max_decompression_ratio,
            }
        );
        Ok(())
    }
}

impl Default for LoadLimits {
    fn default() -> Self {
        Self {
            max_decompression_ratio: Self::DEFAULT_MAX_DECOMPRESSION_RATIO,
        }
    }
}

#[derive(Debug)]
pub(super) struct ExpansionExceeded {
    compressed: u64,
    decoded: u64,
    ratio: u64,
}

impl ExpansionExceeded {
    pub(super) fn description(&self) -> String {
        format!(
            "{} compressed → {} decoded exceeds {}× expansion guard",
            byte_size(self.compressed),
            byte_size(self.decoded),
            self.ratio
        )
    }

    pub(super) fn hint(&self) -> String {
        if self.compressed == 0 {
            "Compressed payload is empty".into()
        } else {
            format!(
                "Required --max-decompression-ratio: {}",
                self.decoded.div_ceil(self.compressed)
            )
        }
    }
}

impl fmt::Display for ExpansionExceeded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.description(), self.hint())
    }
}

impl std::error::Error for ExpansionExceeded {}

#[derive(Debug)]
pub(super) struct SectionError {
    pub(super) name: String,
    path: PathBuf,
    source: anyhow::Error,
}

impl fmt::Display for SectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "reading {} in {}", self.name, self.path.display())
    }
}

impl std::error::Error for SectionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

fn byte_size(bytes: u64) -> String {
    for (scale, unit) in [
        (1 << 60, "EiB"),
        (1 << 50, "PiB"),
        (1 << 40, "TiB"),
        (1 << 30, "GiB"),
        (1 << 20, "MiB"),
        (1 << 10, "KiB"),
    ] {
        if bytes >= scale {
            return if bytes.is_multiple_of(scale) {
                format!("{} {unit}", bytes / scale)
            } else {
                format!("{:.2} {unit}", bytes as f64 / scale as f64)
            };
        }
    }
    format!("{bytes} {}", if bytes == 1 { "byte" } else { "bytes" })
}

pub(crate) struct SectionLoader {
    limits: LoadLimits,
}

impl SectionLoader {
    pub fn new(limits: LoadLimits) -> Self {
        Self { limits }
    }

    fn check_expansion(&self, compressed: u64, decoded: u64) -> Result<()> {
        // Saturation avoids rejecting legitimate sizes when the configured
        // ratio permits more than a u64 can represent.
        self.limits.check_expansion(compressed, decoded)
    }

    /// Check compressed section headers before allocation. Ordinary sections
    /// remain borrowed and are not subject to a size or aggregate budget.
    pub fn read_section<'data>(
        &self,
        section: &impl ObjectSection<'data>,
        path: &Path,
    ) -> Result<Cow<'data, [u8]>> {
        let name = section.name().unwrap_or("<unnamed section>");
        (|| {
            let compressed = section.compressed_data()?;
            let size = compressed.uncompressed_size;
            if compressed.format != CompressionFormat::None {
                self.check_expansion(u64::try_from(compressed.data.len())?, size)?;
            }
            usize::try_from(size).context("decoded section size does not fit in memory")?;
            // object bounds the output buffer and checks the decoded length
            // against the header. Check expansion before it allocates that buffer.
            compressed.decompress().context("decompressing section")
        })()
        .map_err(|source| {
            SectionError {
                name: name.to_owned(),
                path: path.to_owned(),
                source,
            }
            .into()
        })
    }

    pub fn load_section<'data>(
        &self,
        section: &impl ObjectSection<'data>,
        path: &Path,
    ) -> Result<Rc<[u8]>> {
        let bytes = self.read_section(section, path)?;
        Ok(Rc::from(bytes.as_ref()))
    }
}

#[cfg(test)]
pub(super) mod tests;
