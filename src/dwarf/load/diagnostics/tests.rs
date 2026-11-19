use super::*;
use crate::dwarf::load::{
    LoadLimits, SectionLoader,
    limits::tests::{Compression, compressed, section_elf},
};
use object::{Endianness, Object};

fn section_limit_error(path: &Path) -> anyhow::Error {
    let (data, flags) = compressed(b"", 320_954_192, Compression::Zlib, Endianness::Little);
    let bytes = section_elf(&[(".debug_info", data, flags)], Endianness::Little);
    let object = object::File::parse(bytes.as_slice()).unwrap();
    let section = object.section_by_name(".debug_info").unwrap();
    SectionLoader::new(LoadLimits::default())
        .read_section(&section, path)
        .unwrap_err()
}

#[cfg(unix)]
#[test]
fn reports_duplicate_symlink_failures_once_with_readable_sizes() {
    let directory = tempfile::tempdir().unwrap();
    let actual = directory.path().join("library.debug");
    let alias = directory.path().join("build-id.debug");
    std::fs::write(&actual, []).unwrap();
    std::os::unix::fs::symlink(&actual, &alias).unwrap();
    let mut failures = SearchFailures::default();
    failures.candidate(&alias, section_limit_error(&alias));
    failures.candidate(&actual, section_limit_error(&actual));
    let report = crate::text::format_error(&failures.finish(Path::new("/usr/lib64/library.so")));
    assert!(
        report
            .starts_with("no usable debug information found\n  Input:\n    /usr/lib64/library.so")
    );
    assert_eq!(
        report
            .matches(actual.canonicalize().unwrap().to_str().unwrap())
            .count(),
        1
    );
    assert!(!report.contains("build-id.debug"));
    assert_eq!(
        report
            .matches("306.09 MiB decoded exceeds 256× expansion guard")
            .count(),
        1
    );
    assert_eq!(
        report
            .matches("Required --max-decompression-ratio:")
            .count(),
        1
    );
    assert!(!report.contains("reading .debug_info in"));
}

#[test]
fn keeps_distinct_rejection_reasons_and_groups_missing_paths() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("library.debug");
    std::fs::write(&file, []).unwrap();
    let mut failures = SearchFailures::default();
    failures.candidate(&file, anyhow::anyhow!("build ID mismatch"));
    failures.candidate(&file, anyhow::anyhow!("CRC mismatch"));
    for _ in 0..2 {
        failures.candidate(
            Path::new("/missing/library.debug"),
            io::Error::from(io::ErrorKind::NotFound).into(),
        );
    }
    let report = failures.finish(Path::new("/input/library.so")).to_string();
    assert_eq!(
        report
            .matches(file.canonicalize().unwrap().to_str().unwrap())
            .count(),
        1
    );
    assert!(report.contains("      build ID mismatch\n      CRC mismatch"));
    assert!(report.contains("  Not found:\n    /missing/library.debug"));
    assert_eq!(report.matches("/missing/library.debug").count(), 1);
    assert!(!report.contains("os error"));
}
