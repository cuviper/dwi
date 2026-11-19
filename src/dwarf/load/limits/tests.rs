use super::*;
use std::io::Write;

use object::{Architecture, BinaryFormat, Endianness, Object, SectionFlags, SectionKind};

#[derive(Clone, Copy, Debug)]
pub(in crate::dwarf::load) enum Compression {
    Plain,
    Gnu,
    Zlib,
    Zstd,
}

pub(in crate::dwarf::load) fn compressed(
    data: &[u8],
    declared_size: u64,
    compression: Compression,
    endian: Endianness,
) -> (Vec<u8>, u64) {
    if matches!(compression, Compression::Plain) {
        return (data.to_vec(), 0);
    }
    let payload = if matches!(compression, Compression::Zstd) {
        // A single-segment zstd frame containing one raw block.
        assert!(data.len() < 256);
        let mut bytes = vec![0x28, 0xb5, 0x2f, 0xfd, 0x20, data.len() as u8];
        bytes.extend_from_slice(&((data.len() as u32) << 3 | 1).to_le_bytes()[..3]);
        bytes.extend_from_slice(data);
        bytes
    } else {
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(data).unwrap();
        encoder.finish().unwrap()
    };
    let (mut bytes, flags) = if matches!(compression, Compression::Gnu) {
        let mut bytes = b"ZLIB".to_vec();
        bytes.extend_from_slice(&declared_size.to_be_bytes());
        (bytes, 0)
    } else {
        let format = if matches!(compression, Compression::Zstd) {
            2u32
        } else {
            1
        };
        let mut bytes = if endian == Endianness::Little {
            format.to_le_bytes().to_vec()
        } else {
            format.to_be_bytes().to_vec()
        };
        bytes.extend_from_slice(&[0; 4]);
        for value in [declared_size, 1] {
            bytes.extend_from_slice(&if endian == Endianness::Little {
                value.to_le_bytes()
            } else {
                value.to_be_bytes()
            });
        }
        (bytes, object::elf::SHF_COMPRESSED.0)
    };
    bytes.extend_from_slice(&payload);
    (bytes, flags)
}

pub(in crate::dwarf::load) fn section_elf(
    sections: &[(&str, Vec<u8>, u64)],
    endian: Endianness,
) -> Vec<u8> {
    let architecture = if endian == Endianness::Little {
        Architecture::X86_64
    } else {
        Architecture::PowerPc64
    };
    let mut object = object::write::Object::new(BinaryFormat::Elf, architecture, endian);
    for (name, bytes, flags) in sections {
        let id = object.add_section(Vec::new(), name.as_bytes().to_vec(), SectionKind::Debug);
        object.section_mut(id).flags = SectionFlags::Elf {
            sh_type: object::elf::SHT_PROGBITS,
            sh_flags: object::elf::SectionFlags(*flags),
        };
        object.append_section_data(id, bytes, 1);
    }
    object.write().unwrap()
}

#[test]
fn section_loading_covers_compression_formats_and_small_section_allowance() {
    let data = b"bounded section data";
    for endian in [Endianness::Little, Endianness::Big] {
        for compression in [
            Compression::Plain,
            Compression::Gnu,
            Compression::Zlib,
            Compression::Zstd,
        ] {
            let name = if matches!(compression, Compression::Gnu) {
                ".zdebug_str"
            } else {
                ".debug_str"
            };
            let (bytes, flags) = compressed(data, data.len() as u64, compression, endian);
            let bytes = section_elf(&[(name, bytes, flags)], endian);
            let object = object::File::parse(bytes.as_slice()).unwrap();
            let section = object.section_by_name(name).unwrap();
            let path = Path::new("test.debug");
            let loader = SectionLoader::new(LoadLimits {
                max_decompression_ratio: 1,
            });
            for _ in 0..2 {
                assert_eq!(loader.load_section(&section, path).unwrap().as_ref(), data);
            }
        }
    }
}

#[test]
fn oversized_headers_are_rejected_before_decompression() {
    for compression in [Compression::Gnu, Compression::Zlib, Compression::Zstd] {
        let name = if matches!(compression, Compression::Gnu) {
            ".zdebug_str"
        } else {
            ".debug_str"
        };
        // object restricts legacy GNU headers to 32-bit decoded sizes.
        let size = if matches!(compression, Compression::Gnu) {
            u64::from(u32::MAX)
        } else {
            u64::MAX
        };
        let (mut bytes, flags) = compressed(b"", size, compression, Endianness::Little);
        // Keep only the header, leaving a deliberately invalid compressed stream.
        bytes.truncate(if matches!(compression, Compression::Gnu) {
            12
        } else {
            24
        });
        let bytes = section_elf(&[(name, bytes, flags)], Endianness::Little);
        let object = object::File::parse(bytes.as_slice()).unwrap();
        let section = object.section_by_name(name).unwrap();
        let path = Path::new("oversized.debug");
        let loader = SectionLoader::new(LoadLimits::default());
        let error = loader.read_section(&section, path).unwrap_err();
        assert!(error.chain().any(|cause| cause.is::<ExpansionExceeded>()));
        let error = format!("{error:#}");
        assert!(error.contains("expansion guard"), "{error}");
        assert!(error.contains("Compressed payload is empty"), "{error}");
        assert!(
            error.contains(name) && error.contains("oversized.debug"),
            "{error}"
        );
    }
}

#[test]
fn understated_compressed_sizes_do_not_bypass_output_bounds() {
    for compression in [Compression::Gnu, Compression::Zlib, Compression::Zstd] {
        let name = if matches!(compression, Compression::Gnu) {
            ".zdebug_str"
        } else {
            ".debug_str"
        };
        let (bytes, flags) = compressed(&[b'x'; 128], 8, compression, Endianness::Little);
        let bytes = section_elf(&[(name, bytes, flags)], Endianness::Little);
        let object = object::File::parse(bytes.as_slice()).unwrap();
        let section = object.section_by_name(name).unwrap();
        let loader = SectionLoader::new(LoadLimits::default());
        if let Ok(data) = loader.load_section(&section, Path::new("understated.debug")) {
            // The zlib decoder may return a truncated output buffer. Whether
            // accepted or rejected, an understated header must not grow output.
            assert!(data.len() <= 8);
        }
    }
}

#[test]
fn uncompressed_sections_are_borrowed_without_size_or_aggregate_caps() {
    let size = LoadLimits::SMALL_SECTION_ALLOWANCE as usize + 1;
    let bytes = section_elf(&[(".debug_sup", vec![0; size], 0)], Endianness::Little);
    let object = object::File::parse(bytes.as_slice()).unwrap();
    let section = object.section_by_name(".debug_sup").unwrap();
    let loader = SectionLoader::new(LoadLimits {
        max_decompression_ratio: 1,
    });
    let path = Path::new("temporary.debug");
    for _ in 0..2 {
        let data = loader.read_section(&section, path).unwrap();
        assert!(matches!(data, Cow::Borrowed(_)));
        assert_eq!(data.len(), size);
    }
}

#[test]
fn ratio_guard_handles_allowance_boundaries_overrides_and_overflow() {
    let loader = SectionLoader::new(LoadLimits::default());
    let mib = 1024 * 1024;
    assert!(loader.check_expansion(1, 16 * mib).is_ok());
    assert!(loader.check_expansion(1, 16 * mib + 1).is_err());
    assert!(loader.check_expansion(mib, 256 * mib).is_ok());
    let error = loader.check_expansion(mib, 256 * mib + 1).unwrap_err();
    let limit = error.downcast_ref::<ExpansionExceeded>().unwrap();
    assert_eq!(limit.hint(), "Required --max-decompression-ratio: 257");
    let generous = SectionLoader::new(LoadLimits {
        max_decompression_ratio: 512,
    });
    assert!(generous.check_expansion(mib, 256 * mib + 1).is_ok());
    let huge = SectionLoader::new(LoadLimits {
        max_decompression_ratio: u64::MAX,
    });
    assert!(huge.check_expansion(2, u64::MAX).is_ok());
    // Representative compressed/decoded sizes from the large rustc debug file.
    for (compressed, decoded) in [
        (123 * mib, 306 * mib),
        (24 * mib, 302 * mib),
        (44 * mib, 415 * mib),
    ] {
        assert!(loader.check_expansion(compressed, decoded).is_ok());
    }
}

#[test]
fn decoded_size_mismatches_still_fail_within_the_allowance() {
    for compression in [Compression::Gnu, Compression::Zlib, Compression::Zstd] {
        let name = if matches!(compression, Compression::Gnu) {
            ".zdebug_str"
        } else {
            ".debug_str"
        };
        let (bytes, flags) = compressed(b"short", 128, compression, Endianness::Little);
        let bytes = section_elf(&[(name, bytes, flags)], Endianness::Little);
        let object = object::File::parse(bytes.as_slice()).unwrap();
        let section = object.section_by_name(name).unwrap();
        let loader = SectionLoader::new(LoadLimits::default());
        let error = loader
            .read_section(&section, Path::new("mismatch.debug"))
            .unwrap_err();
        assert!(!error.chain().any(|cause| cause.is::<ExpansionExceeded>()));
        assert!(format!("{error:#}").contains("decompressing section"));
    }
}
