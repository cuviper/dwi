use super::*;
use crate::dwarf::load::tests::{elf, elf_with_build_id};

const RELEASE: &str = "7.3.4-300.test.x86_64";
const ID: &[u8] = &[0xab, 0xcd, 0xef, 0x12, 0x34];

struct Fixture {
    _directory: tempfile::TempDir,
    root: PathBuf,
    debug: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        Self {
            debug: root.join("usr/lib/debug"),
            root,
            _directory: directory,
        }
    }

    fn write(&self, path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn discover(&self, id: Option<&[u8]>) -> Result<(Database, PathBuf)> {
        discover(
            RELEASE,
            id,
            &self.root,
            &[&self.debug],
            LoadLimits::default(),
            SearchFailures::default(),
        )
    }
}

#[test]
fn discovers_release_specific_debug_paths_in_order() {
    let fixture = Fixture::new();
    let first = fixture
        .debug
        .join("lib/modules")
        .join(RELEASE)
        .join("vmlinux");
    let second = fixture
        .debug
        .join("usr/lib/modules")
        .join(RELEASE)
        .join("vmlinux");
    fixture.write(&first, &elf(Some("first.c"), None, Endianness::Little));
    fixture.write(&second, &elf(Some("second.c"), None, Endianness::Little));
    let (mut db, path) = fixture.discover(None).unwrap();
    assert_eq!(path, first);
    assert_eq!(db.die(db.roots[0]).unwrap().name, "first.c");
    fs::remove_file(&first).unwrap();
    let (mut db, path) = fixture.discover(None).unwrap();
    assert_eq!(path, second);
    assert_eq!(db.die(db.roots[0]).unwrap().name, "second.c");
    assert!(db.debug_file.is_none());
}

#[test]
fn running_build_id_takes_precedence_and_verifies_conventional_paths() {
    let fixture = Fixture::new();
    let by_id = fixture.debug.join(".build-id/ab/cdef1234.debug");
    let named = fixture
        .debug
        .join("lib/modules")
        .join(RELEASE)
        .join("vmlinux");
    fixture.write(
        &by_id,
        &elf_with_build_id(Some("by-id.c"), None, Some(ID), Endianness::Little),
    );
    fixture.write(
        &named,
        &elf_with_build_id(Some("named.c"), None, Some(ID), Endianness::Little),
    );
    let (mut db, path) = fixture.discover(Some(ID)).unwrap();
    assert_eq!(path, by_id);
    assert_eq!(db.die(db.roots[0]).unwrap().name, "by-id.c");
    assert!(db.debug_file.is_none());
    fixture.write(
        &by_id,
        &elf_with_build_id(Some("wrong.c"), None, Some(b"wrong"), Endianness::Little),
    );
    assert_eq!(fixture.discover(Some(ID)).unwrap().1, named);
    fixture.write(&named, &elf(Some("no-id.c"), None, Endianness::Little));
    let error = fixture.discover(Some(ID)).err().unwrap().to_string();
    assert!(error.contains("build ID mismatch"));
    assert!(error.contains("no GNU build ID"));
    assert!(error.contains(&format!("Install kernel debuginfo for {RELEASE}")));
}

#[test]
fn selected_kernel_reuses_debuglink_loading() {
    let fixture = Fixture::new();
    let image = fixture
        .root
        .join("lib/modules")
        .join(RELEASE)
        .join("vmlinux");
    let debug = image.with_file_name("vmlinux.debug");
    let bytes = elf(Some("kernel.c"), None, Endianness::Little);
    fixture.write(&debug, &bytes);
    let mut link = b"vmlinux.debug\0".to_vec();
    link.resize(link.len().next_multiple_of(4), 0);
    link.extend_from_slice(&crc32fast::hash(&bytes).to_le_bytes());
    fixture.write(&image, &elf(None, Some(&link), Endianness::Little));
    let (mut db, path) = fixture.discover(None).unwrap();
    assert_eq!(path, image);
    assert_eq!(db.debug_file.as_deref(), Some(debug.as_path()));
    assert_eq!(db.die(db.roots[0]).unwrap().name, "kernel.c");
}

#[test]
fn a_matching_kernel_load_failure_stops_discovery() {
    let fixture = Fixture::new();
    let by_id = fixture.debug.join(".build-id/ab/cdef1234.debug");
    let named = fixture
        .debug
        .join("lib/modules")
        .join(RELEASE)
        .join("vmlinux");
    fixture.write(
        &by_id,
        &elf_with_build_id(None, None, Some(ID), Endianness::Little),
    );
    fixture.write(
        &named,
        &elf_with_build_id(Some("usable.c"), None, Some(ID), Endianness::Little),
    );
    let error = format!("{:#}", fixture.discover(Some(ID)).err().unwrap());
    assert!(error.contains("file contains no DWARF units"));
    assert!(!error.contains(named.to_str().unwrap()));
}

#[test]
fn only_the_requested_release_is_considered() {
    let fixture = Fixture::new();
    fixture.write(
        &fixture.debug.join("usr/lib/modules/other-release/vmlinux"),
        &elf(Some("other.c"), None, Endianness::Little),
    );
    let error = fixture.discover(None).err().unwrap().to_string();
    assert!(error.contains(RELEASE));
    assert!(error.contains("Install kernel debuginfo"));
    assert!(!error.contains("other-release"));
    for invalid in ["", ".", "..", "../other", "one/two", "release/"] {
        assert!(validate_release(invalid).is_err(), "{invalid}");
    }
    assert!(validate_release(RELEASE).is_ok());
}

#[test]
fn kernel_symlinks_are_displayed_as_canonical_paths() {
    let fixture = Fixture::new();
    let image = fixture
        .root
        .join("lib/modules")
        .join(RELEASE)
        .join("vmlinux");
    let actual = fixture.root.join("images/vmlinux");
    fixture.write(&actual, &elf(Some("kernel.c"), None, Endianness::Little));
    fs::create_dir_all(image.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&actual, &image).unwrap();
    assert_eq!(fixture.discover(None).unwrap().1, actual);
}

fn note(owner: &[u8], kind: u32, descriptor: &[u8], endian: Endianness) -> Vec<u8> {
    let mut bytes = Vec::new();
    for value in [owner.len() as u32, descriptor.len() as u32, kind] {
        bytes.extend_from_slice(&if endian == Endianness::Little {
            value.to_le_bytes()
        } else {
            value.to_be_bytes()
        });
    }
    bytes.extend_from_slice(owner);
    bytes.resize(bytes.len().next_multiple_of(4), 0);
    bytes.extend_from_slice(descriptor);
    bytes.resize(bytes.len().next_multiple_of(4), 0);
    bytes
}

#[test]
fn raw_kernel_notes_find_gnu_build_ids_in_both_endiannesses() {
    for endian in [Endianness::Little, Endianness::Big] {
        let mut bytes = note(b"Linux\0", 3, b"release", endian);
        bytes.extend(note(b"Xen\0", 3, b"not a GNU ID", endian));
        assert_eq!(notes_build_id(&bytes, endian).unwrap(), None);
        bytes.extend(note(b"GNU\0", 3, ID, endian));
        assert_eq!(notes_build_id(&bytes, endian).unwrap(), Some(ID.to_vec()));
        assert!(notes_build_id(&[0; 8], endian).is_err());
        assert!(notes_build_id(&note(b"GNU\0", 3, &[], endian), endian).is_err());
    }
}

#[test]
fn installed_releases_do_not_use_the_running_kernels_notes() {
    let fixture = Fixture::new();
    let notes = fixture.root.join("notes");
    fixture.write(&notes, b"malformed notes");
    assert_eq!(
        running_build_id("installed", "running", &notes).unwrap(),
        None
    );
    assert!(running_build_id("running", "running", &notes).is_err());
    let endian = if cfg!(target_endian = "little") {
        Endianness::Little
    } else {
        Endianness::Big
    };
    fixture.write(&notes, &note(b"GNU\0", 3, ID, endian));
    assert_eq!(
        running_build_id("running", "running", &notes).unwrap(),
        Some(ID.to_vec())
    );
    fs::remove_file(notes.clone()).unwrap();
    assert_eq!(
        running_build_id("running", "running", &notes).unwrap(),
        None
    );
}
