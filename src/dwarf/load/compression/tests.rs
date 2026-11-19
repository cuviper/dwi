use super::*;
use crate::dwarf::{Database, load::tests::elf};
use object::Endianness;
use std::io::Write;

#[derive(Clone, Copy)]
pub(in crate::dwarf::load) enum Codec {
    Plain,
    Gzip,
    Xz,
    Zstd,
}

impl Codec {
    pub(in crate::dwarf::load) fn suffix(self) -> &'static str {
        match self {
            Self::Plain => "",
            Self::Gzip => ".gz",
            Self::Xz => ".xz",
            Self::Zstd => ".zst",
        }
    }
}

pub(in crate::dwarf::load) fn encode(bytes: &[u8], codec: Codec) -> Vec<u8> {
    match codec {
        Codec::Plain => bytes.to_vec(),
        Codec::Gzip => {
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
            encoder.write_all(bytes).unwrap();
            encoder.finish().unwrap()
        }
        Codec::Xz => {
            let mut encoder =
                lzma_rust2::XzWriter::new(Vec::new(), lzma_rust2::XzOptions::with_preset(1))
                    .unwrap();
            encoder.write_all(bytes).unwrap();
            encoder.finish().unwrap()
        }
        Codec::Zstd => {
            // Single-segment frame, four-byte content size, one raw block.
            assert!(bytes.len() < 128 * 1024);
            let mut frame = vec![0x28, 0xb5, 0x2f, 0xfd, 0xa0];
            frame.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            frame.extend_from_slice(&((bytes.len() as u32) << 3 | 1).to_le_bytes()[..3]);
            frame.extend_from_slice(bytes);
            frame
        }
    }
}

#[test]
fn compressed_module_files_can_be_opened_directly() {
    let directory = tempfile::tempdir().unwrap();
    let original = elf(Some("module.c"), None, Endianness::Little);
    for codec in [Codec::Plain, Codec::Gzip, Codec::Xz, Codec::Zstd] {
        let path = directory
            .path()
            .join(format!("module.ko{}", codec.suffix()));
        fs::write(&path, encode(&original, codec)).unwrap();
        let mut db = Database::open(&path, LoadLimits::default()).unwrap();
        assert_eq!(db.die(db.roots[0]).unwrap().name, "module.c");
    }
}

#[test]
fn truncated_compressed_modules_report_errors() {
    let original = elf(Some("module.c"), None, Endianness::Little);
    for codec in [Codec::Gzip, Codec::Xz, Codec::Zstd] {
        let mut bytes = encode(&original, codec);
        bytes.pop();
        assert!(
            decode(bytes, LoadLimits::default()).is_err(),
            "{}",
            codec.suffix()
        );
    }
}

#[test]
fn streaming_decompression_obeys_the_existing_expansion_allowance() {
    let original = vec![0; 17 * 1024 * 1024];
    let bytes = encode(&original, Codec::Gzip);
    let error = decode(
        bytes.clone(),
        LoadLimits {
            max_decompression_ratio: 1,
        },
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("--max-decompression-ratio"));
    assert_eq!(decode(bytes, LoadLimits::default()).unwrap(), original);
}
