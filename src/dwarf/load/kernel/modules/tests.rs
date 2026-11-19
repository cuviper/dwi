use super::*;
use crate::dwarf::load::{
    compression::tests::{Codec, encode},
    tests::{elf, elf_with_build_id},
};
use object::Endianness;

const RELEASE: &str = "7.3.4-test";
const ID: &[u8] = &[0xab, 0xcd, 0xef, 0x12, 0x34];

struct Fixture {
    _directory: tempfile::TempDir,
    root: PathBuf,
    modules: PathBuf,
    debug: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        Self {
            modules: root.join("usr/lib/modules").join(RELEASE),
            debug: root.join("usr/lib/debug"),
            root,
            _directory: directory,
        }
    }
    fn write(&self, path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    fn open(&self, name: &str) -> Result<(Database, PathBuf)> {
        open(
            name,
            RELEASE,
            &self.root,
            &[&self.debug],
            LoadLimits::default(),
        )
    }
}

#[test]
fn dep_index_resolves_normalized_names_and_compressed_build_ids() {
    for codec in [Codec::Plain, Codec::Gzip, Codec::Xz, Codec::Zstd] {
        let fixture = Fixture::new();
        let relative = format!("kernel/drivers/net/test-net.ko{}", codec.suffix());
        let path = fixture.modules.join(&relative);
        fixture.write(
            &fixture.modules.join("modules.dep"),
            format!("{relative}: kernel/dependency.ko\n").as_bytes(),
        );
        fixture.write(
            &path,
            &encode(
                &elf_with_build_id(None, None, Some(ID), Endianness::Little),
                codec,
            ),
        );
        let debug = fixture.debug.join(".build-id/ab/cdef1234.debug");
        fixture.write(
            &debug,
            &elf_with_build_id(Some("test-net.c"), None, Some(ID), Endianness::Little),
        );
        let (mut db, actual) = fixture.open("test_net").unwrap();
        assert_eq!(actual, path);
        assert_eq!(db.debug_file.as_deref(), Some(debug.as_path()));
        assert_eq!(db.die(db.roots[0]).unwrap().name, "test-net.c");
        assert!(fixture.open("dependency").is_err());
    }
}

#[test]
fn compressed_modules_reuse_debuglink_lookup() {
    let fixture = Fixture::new();
    let relative = "kernel/test.ko.xz";
    let path = fixture.modules.join(relative);
    let debug = path.with_file_name("test.ko.debug");
    let bytes = elf(Some("test.c"), None, Endianness::Little);
    fixture.write(&debug, &bytes);
    let mut link = b"test.ko.debug\0".to_vec();
    link.resize(link.len().next_multiple_of(4), 0);
    link.extend_from_slice(&crc32fast::hash(&bytes).to_le_bytes());
    fixture.write(
        &path,
        &encode(&elf(None, Some(&link), Endianness::Little), Codec::Xz),
    );
    fixture.write(
        &fixture.modules.join("modules.dep"),
        format!("{relative}:\n").as_bytes(),
    );
    let (db, actual) = fixture.open("test").unwrap();
    assert_eq!(actual, path);
    assert_eq!(db.debug_file.as_deref(), Some(debug.as_path()));
}

#[test]
fn builtins_explain_how_to_browse_vmlinux() {
    let fixture = Fixture::new();
    fixture.write(
        &fixture.modules.join("modules.builtin"),
        b"kernel/fs/built-in.ko\n",
    );
    let error = fixture.open("built_in").err().unwrap().to_string();
    assert!(error.contains("is built into kernel 7.3.4-test"));
    assert!(error.contains("dwi --kernel 7.3.4-test"));
}

#[test]
fn filesystem_fallback_supports_debuginfo_only_and_reports_ambiguity() {
    let fixture = Fixture::new();
    let first = fixture
        .debug
        .join("usr/lib/modules")
        .join(RELEASE)
        .join("kernel/test.ko.debug");
    let second = fixture
        .debug
        .join("usr/lib/modules")
        .join(RELEASE)
        .join("updates/test.ko.debug");
    fixture.write(&first, &elf(Some("first.c"), None, Endianness::Little));
    let (mut db, actual) = fixture.open("test").unwrap();
    assert_eq!(actual, first);
    assert_eq!(db.die(db.roots[0]).unwrap().name, "first.c");
    fixture.write(&second, &elf(Some("second.c"), None, Endianness::Little));
    let error = fixture.open("test").err().unwrap().to_string();
    assert!(error.contains("multiple modules named test"));
    assert!(error.contains(first.to_str().unwrap()));
    assert!(error.contains(second.to_str().unwrap()));
}

#[test]
fn module_index_order_is_authoritative_even_when_loading_fails() {
    let fixture = Fixture::new();
    let first = fixture.modules.join("updates/test.ko");
    let second = fixture.modules.join("kernel/test.ko");
    fixture.write(
        &fixture.modules.join("modules.dep"),
        b"updates/test.ko:\nkernel/test.ko:\n",
    );
    fixture.write(&first, &elf(Some("first.c"), None, Endianness::Little));
    fixture.write(&second, &elf(Some("second.c"), None, Endianness::Little));
    assert_eq!(fixture.open("test").unwrap().1, first);
    fixture.write(&first, b"not an ELF file");
    let error = format!("{:#}", fixture.open("test").err().unwrap());
    assert!(error.contains(first.to_str().unwrap()));
    assert!(!error.contains(second.to_str().unwrap()));
}

#[test]
fn module_symlinks_and_missing_indices_work_without_traversing_source_trees() {
    let fixture = Fixture::new();
    let actual = fixture.root.join("elsewhere/module.ko.xz");
    fixture.write(
        &actual,
        &encode(&elf(Some("module.c"), None, Endianness::Little), Codec::Xz),
    );
    let alias = fixture.modules.join("weak-updates/test.ko.xz");
    fs::create_dir_all(alias.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&actual, &alias).unwrap();
    std::os::unix::fs::symlink(&fixture.root, fixture.modules.join("source")).unwrap();
    assert_eq!(fixture.open("test").unwrap().1, actual);
}

#[test]
fn module_lookup_validates_names_and_stays_within_the_requested_release() {
    let fixture = Fixture::new();
    fixture.write(
        &fixture.root.join("usr/lib/modules/other/kernel/test.ko"),
        &elf(Some("other.c"), None, Endianness::Little),
    );
    let error = fixture.open("test").err().unwrap().to_string();
    assert!(error.contains("module test not found for kernel 7.3.4-test"));
    assert!(!error.contains("other/kernel"));
    for invalid in ["", ".", "..", "../test", "path/test.ko", ".ko", "pci:123"] {
        assert!(normalize_name(invalid).is_err(), "{invalid}");
    }
    assert_eq!(normalize_name("test-net.ko.xz").unwrap(), "test_net");
}
